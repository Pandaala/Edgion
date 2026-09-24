/// Isolate actual request-entry tests without mutating a process-global snapshot.
/// The guard must remain on its creating thread, including across async awaits.
#[cfg(test)]
pub(crate) struct TestListenerIndexGuard {
    key: ListenerKey,
    previous: Option<Arc<GatewayInfoIndex>>,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
