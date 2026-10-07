//! Scoped synchronous mechanism measurements; absent from production builds.
use std::cell::RefCell;

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Event {
    GroupScratchInitializations,
    BooleanScratchInitializations,
    CaptureScratchInitializations,
    AvoidedPredicateGroups,
    GrammarInstances,
    SourceBytes,
    Completions,
    TokenCopyBytes,
    KeyCopyBytes,
    PathCopyBytes,
    NameCopyBytes,
    DeferredCopyBytes,
    MatchedCopyBytes,
    CaptureCopyBytes,
    TemporaryCaptureCopyBytes,
    UnitTxClones,
    Candidates,
    DispatchGroups,
    DispatchWork,
    ProfileWalks,
    TransformSteps,
    OperatorStages,
    FullScans,
    RangeQueries,
    RangeQueryFailures,
    TransformWork,
    OperatorWork,
    DetectorWork,
    LexicalPeakBytes,
    StructurePeakBytes,
    LexicalWork,
    StructureWork,
}
const COUNT: usize = Event::StructureWork as usize + 1;
#[derive(Clone, Copy)]
pub(crate) struct Counts([usize; COUNT]);
impl Counts {
    pub(crate) fn get(self, event: Event) -> usize {
        self.0[event as usize]
    }
}
thread_local! {
    static ACTIVE: RefCell<Option<Counts>> = const { RefCell::new(None) };
}
pub(crate) fn record(event: Event, amount: usize) {
    ACTIVE.with_borrow_mut(|active| {
        if let Some(counts) = active {
            let count = &mut counts.0[event as usize];
            *count = count.checked_add(amount).expect("measurement counter overflow");
        }
    });
}
pub(crate) struct Scope {
    previous: Option<Counts>,
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
pub(crate) fn start() -> Scope {
    Scope {
        previous: ACTIVE.with_borrow_mut(|active| active.replace(Counts([0; COUNT]))),
        thread_bound: std::marker::PhantomData,
    }
}
impl Scope {
    pub(crate) fn counts(&self) -> Counts {
        ACTIVE.with_borrow(|active| active.expect("measurement scope must be active"))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with_borrow_mut(|active| *active = self.previous.take());
    }
}
#[test]
fn measurement_scope_restores_previous_state_after_panic() {
    let outer = start();
    record(Event::SourceBytes, 7);
    assert!(std::panic::catch_unwind(|| {
        let _inner = start();
        record(Event::SourceBytes, 13);
        panic!("intentional scope unwind");
    })
    .is_err());
    assert_eq!(outer.counts().get(Event::SourceBytes), 7);
    drop(outer);
    assert!(ACTIVE.with_borrow(Option::is_none));
}
