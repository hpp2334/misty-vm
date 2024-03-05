use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use misty_vm::{
    client::SingletonMistyClientPod,
    controllers::{ControllerRet, MistyController},
    resources::{MistyResourceId, ResourceUpdateAction},
    services::MistyServiceManager,
    signals::MistySignal,
    states::MistyStateManager,
    views::MistyViewModelManager,
};

#[derive(Clone)]
pub struct TestAppContainer<R>
where
    R: Clone + Default,
{
    app: Arc<SingletonMistyClientPod<R>>,
    state: Arc<Mutex<R>>,
    resources: Arc<Mutex<HashMap<MistyResourceId, Vec<u8>>>>,
    apply_view: Arc<dyn Fn(R, &mut R) + Send + Sync + 'static>,
}
pub struct TestApp<R>
where
    R: Clone + Default,
{
    app: TestAppContainer<R>,
}

impl<R> TestAppContainer<R>
where
    R: Default + Clone + Send + Sync + 'static,
{
    pub fn new(apply_view: impl Fn(R, &mut R) + Send + Sync + 'static) -> Self {
        Self {
            app: Arc::new(SingletonMistyClientPod::new()),
            state: Default::default(),
            resources: Default::default(),
            apply_view: Arc::new(apply_view),
        }
    }

    pub fn call_controller<Controller, Arg, E>(&self, controller: Controller, arg: Arg)
    where
        Controller: MistyController<Arg, E>,
        E: std::fmt::Debug,
    {
        let ret = self.app.call_controller(controller, arg).unwrap();
        self.apply(ret);
    }

    pub fn flush_schedules(&self) {
        let ret = self.app.flush_scheduled_tasks().unwrap();
        self.apply(ret);
    }

    pub fn get_resource(&self, id: MistyResourceId) -> Option<Vec<u8>> {
        let w = self.resources.lock().unwrap();
        w.get(&id).map(|r| r.clone())
    }

    fn apply(&self, ret: ControllerRet<R>) {
        {
            let mut state_guard = self.state.lock().unwrap();
            if let Some(changed_view) = ret.changed_view {
                (self.apply_view)(changed_view, &mut state_guard);
            }
        }

        {
            let mut w = self.resources.lock().unwrap();

            for resource in ret.changed_resources.into_iter() {
                match resource {
                    ResourceUpdateAction::Insert(id, buf) => {
                        w.insert(id, buf);
                    }
                    ResourceUpdateAction::Remove(id) => {
                        w.remove(&id);
                    }
                }
            }
        }
    }
}
impl<R> TestApp<R>
where
    R: Default + Clone + Send + Sync + 'static,
{
    pub fn new(
        view_manager: MistyViewModelManager<R>,
        service_manager: MistyServiceManager,
        state_manager: MistyStateManager,
        app_container: TestAppContainer<R>,
    ) -> Self {
        app_container
            .app
            .create(view_manager, state_manager, service_manager);

        let cloned = app_container.clone();
        app_container.app.on_signal(move |signal| match signal {
            MistySignal::Schedule => {
                cloned.flush_schedules();
            }
        });

        Self { app: app_container }
    }

    pub fn app(&self) -> TestAppContainer<R> {
        self.app.clone()
    }

    pub fn state(&self) -> R {
        self.app().state.lock().unwrap().clone()
    }
}
