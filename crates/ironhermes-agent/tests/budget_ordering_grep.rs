//! E-05 / AI-SPEC Pitfall 9: `BudgetHandle` MUST NOT use `Ordering::Relaxed`
//! on the shared parent/child counter. `SeqCst` everywhere keeps the pressure
//! tier transitions well-defined across cores.

#[test]
fn budget_uses_only_seqcst_ordering() {
    let src = include_str!("../src/budget.rs");
    assert!(
        !src.contains("Ordering::Relaxed"),
        "E-05 / AI-SPEC Pitfall 9: BudgetHandle MUST NOT use Relaxed ordering on the shared parent/child counter. Use SeqCst everywhere."
    );
    // Must contain at least one SeqCst reference (shell has it).
    assert!(
        src.contains("Ordering::SeqCst"),
        "E-05: BudgetHandle MUST use Ordering::SeqCst."
    );
}
