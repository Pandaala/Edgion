use super::*;
#[test]
fn exchange_and_all_output_clones_jointly_retain_one_reservation() {
    let budget = UnaryBudget::new(1);
    let exchange = budget.acquire().unwrap();
    let bytes = output(vec![1, 2, 3], exchange.clone());
    let clone = bytes.clone();
    let slice = bytes.slice(1..);
    assert!(budget.acquire().is_none());
    drop(bytes);
    drop(clone);
    drop(slice);
    assert!(budget.acquire().is_none());
    drop(exchange);
    assert!(budget.acquire().is_some());
    let exchange = budget.acquire().unwrap();
    let bytes = output(vec![1], exchange.clone());
    drop(exchange);
    assert!(budget.acquire().is_none());
    drop(bytes);
    assert!(budget.acquire().is_some());
}
