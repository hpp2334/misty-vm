use std::{
    any::TypeId,
    collections::HashMap,
    marker::PhantomData,
    sync::{atomic::AtomicU64, Arc, RwLock, Weak},
};

use once_cell::sync::Lazy;

use crate::{
    client::{
        AsMistyClientHandle, AsReadonlyMistyClientHandle, MistyClientAccessor, MistyClientHandle,
        MistyClientInner, MistyReadonlyClientHandle,
    },
    utils::PhantomUnsync,
};

static ASYNC_RT: Lazy<tokio::runtime::Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
});

pub struct MistyAsyncTaskContext {
    pub(crate) inner: Weak<MistyClientInner>,
}

pub struct MistyClientAsyncHandleGuard {
    inner: Option<Arc<MistyClientInner>>,
    _unsync_marker: PhantomUnsync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MistyAsyncTaskId(u64);

type InternalMistyTaskSpawnedHandle = tokio::task::JoinHandle<()>;

#[derive(Debug)]
pub struct MistyAsyncTaskHandle<T> {
    _id: MistyAsyncTaskId,
    marker: PhantomData<T>,
}

#[derive(Debug, Default)]
struct InternalMistyAsyncTaskPool {
    internal_handles: HashMap<MistyAsyncTaskId, InternalMistyTaskSpawnedHandle>,
}

#[derive(Debug, Clone, Default)]
struct BoxedMistyAsyncTaskPool {
    pool: Arc<RwLock<InternalMistyAsyncTaskPool>>,
}

#[derive(Debug)]
struct MistyAsyncTaskPool<T> {
    pool: Arc<RwLock<InternalMistyAsyncTaskPool>>,
    _marker: PhantomData<T>,
}

type InternalMistyAsyncTaskPools = HashMap<TypeId, BoxedMistyAsyncTaskPool>;

#[derive(Debug)]
pub(crate) struct MistyAsyncTaskPools {
    pools: Arc<RwLock<InternalMistyAsyncTaskPools>>,
}

struct MistyAsyncTaskPoolSpawnCleanupGuard<T>
where
    T: MistyAsyncTaskTrait,
{
    task_id: MistyAsyncTaskId,
    marker: PhantomData<T>,
    pools: Weak<RwLock<InternalMistyAsyncTaskPools>>,
}

const _: () = {
    static ALLOCATED: AtomicU64 = AtomicU64::new(1);
    impl MistyAsyncTaskId {
        pub fn alloc() -> Self {
            let id = ALLOCATED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Self(id)
        }
    }
};

impl<T> Drop for MistyAsyncTaskPoolSpawnCleanupGuard<T>
where
    T: MistyAsyncTaskTrait,
{
    fn drop(&mut self) {
        if let Some(pools) = self.pools.upgrade() {
            let tid = std::any::TypeId::of::<T>();
            let pool = pools.write().unwrap();
            let pool = pool.get(&tid);
            if let Some(pool) = pool {
                let mut pool = pool.pool.write().unwrap();
                pool.internal_handles.remove(&self.task_id);
            }
        }
    }
}

impl MistyAsyncTaskPools {
    pub fn new() -> Self {
        Self {
            pools: Default::default(),
        }
    }

    fn get<T: 'static>(&self) -> MistyAsyncTaskPool<T> {
        let pool = {
            let mut pools = self.pools.write().unwrap();
            let pool = pools
                .entry(std::any::TypeId::of::<T>())
                .or_default()
                .clone();
            pool
        };

        MistyAsyncTaskPool {
            pool: pool.pool.clone(),
            _marker: Default::default(),
        }
    }

    pub(crate) fn reset(&self) {
        let mut pools = self.pools.write().unwrap();

        for (_, pool) in pools.iter() {
            let mut pool = pool.pool.write().unwrap();
            for (_, handle) in pool.internal_handles.iter() {
                handle.abort();
            }
            pool.internal_handles.clear();
        }
        pools.clear();
    }
}

impl<T> MistyAsyncTaskPool<T>
where
    T: MistyAsyncTaskTrait,
{
    pub fn spawn<R, E>(
        &self,
        handle: MistyReadonlyClientHandle,
        future_fn: impl (FnOnce(MistyAsyncTaskContext) -> R) + Send + Sync + 'static,
    ) -> MistyAsyncTaskHandle<T>
    where
        R: std::future::Future<Output = Result<(), E>> + Send + 'static,
        E: std::fmt::Display,
    {
        let inner = handle.inner.clone();
        let _guard = ASYNC_RT.enter();
        let task_id = MistyAsyncTaskId::alloc();

        let handle = tokio::spawn(async move {
            let _guard = MistyAsyncTaskPoolSpawnCleanupGuard::<T> {
                task_id,
                marker: Default::default(),
                pools: Arc::downgrade(&inner.async_task_pools.pools),
            };

            let ctx = MistyAsyncTaskContext::new(Arc::downgrade(&inner));
            let res = future_fn(ctx).await;
            if res.is_err() {
                let e = res.unwrap_err();
                tracing::error!("spawn error: {}", e);
            }
        });
        {
            let mut pool = self.pool.write().unwrap();
            pool.internal_handles.insert(task_id, handle);
        }

        MistyAsyncTaskHandle {
            _id: task_id,
            marker: Default::default(),
        }
    }

    pub async fn wait_all(&self) {
        loop {
            let internal_handles = {
                let mut ret: Vec<InternalMistyTaskSpawnedHandle> = Default::default();
                let mut pool = self.pool.write().unwrap();
                let ids: Vec<MistyAsyncTaskId> = pool
                    .internal_handles
                    .keys()
                    .into_iter()
                    .map(|v| *v)
                    .collect();
                for id in ids.into_iter() {
                    ret.push(pool.internal_handles.remove(&id).unwrap());
                }
                pool.internal_handles.clear();
                ret
            };
            tracing::info!("wait all handles {}", internal_handles.len());
            if internal_handles.is_empty() {
                break;
            }

            for handle in internal_handles.into_iter() {
                handle.abort();
            }
        }
    }

    pub fn cancel_all(&self) {
        let mut pool = self.pool.write().unwrap();

        for (_, handle) in pool.internal_handles.iter() {
            handle.abort();
        }
        pool.internal_handles.clear();
    }
}

impl MistyClientAsyncHandleGuard {
    pub fn handle(&self) -> MistyReadonlyClientHandle {
        // SAFETY: spawned task will be aborted when client destroyed
        let inner = self.inner.as_ref().unwrap();
        MistyReadonlyClientHandle { inner }
    }
}

impl MistyAsyncTaskContext {
    fn new(inner: Weak<MistyClientInner>) -> Self {
        Self { inner }
    }

    pub fn handle(&self) -> MistyClientAsyncHandleGuard {
        let inner = self.inner.upgrade();
        MistyClientAsyncHandleGuard {
            inner,
            _unsync_marker: Default::default(),
        }
    }

    pub fn accessor(&self) -> MistyClientAccessor {
        MistyClientAccessor {
            inner: self.inner.clone(),
        }
    }

    pub fn schedule<E>(
        &self,
        handler: impl FnOnce(MistyClientHandle) -> Result<(), E> + Send + Sync + 'static,
    ) where
        E: std::fmt::Display,
    {
        let client = self.inner.upgrade();
        if client.is_none() {
            return;
        }
        let inner = client.unwrap();

        if inner.is_destroyed() {
            tracing::warn!("schedule but client is destroyed");
            return;
        }
        inner
            .schedule_manager
            .enqueue(&inner.signal_emitter, handler);
    }
}

pub trait MistyAsyncTaskTrait: Sized + Send + Sync + 'static {
    fn spawn_once<'a, T, E>(
        cx: impl AsMistyClientHandle<'a>,
        future_fn: impl (FnOnce(MistyAsyncTaskContext) -> T) + Send + Sync + 'static,
    ) where
        T: std::future::Future<Output = Result<(), E>> + Send + 'static,
        E: std::fmt::Display,
    {
        let pool = cx.handle().inner.async_task_pools.get::<Self>();
        pool.cancel_all();
        pool.spawn(cx.readonly_handle().clone(), future_fn);
    }

    fn spawn<'a, T, E>(
        cx: impl AsMistyClientHandle<'a>,
        future_fn: impl (FnOnce(MistyAsyncTaskContext) -> T) + Send + Sync + 'static,
    ) where
        T: std::future::Future<Output = Result<(), E>> + Send + 'static,
        E: std::fmt::Display,
    {
        let pool = cx.handle().inner.async_task_pools.get::<Self>();
        pool.spawn(cx.readonly_handle(), future_fn);
    }

    fn cancel_all<'a>(cx: impl AsMistyClientHandle<'a>) {
        let pool = cx.handle().inner.async_task_pools.get::<Self>();
        pool.cancel_all();
    }

    fn wait_all<'a>(cx: &MistyAsyncTaskContext) -> impl std::future::Future<Output = ()> + Send {
        let handle = cx.handle();
        let handle = handle.handle();
        let pool = handle.inner.async_task_pools.get::<Self>();
        async move {
            pool.wait_all().await;
        }
    }
}
