use super::*;
#[test]
fn output_credits_follow_the_last_bytes_reference_and_charge_capacity() {
    let admission = Arc::new(Semaphore::new(1));
    let reservation = Reservation::acquire_from(&admission).unwrap();
    assert!(Reservation::acquire_from(&admission).is_none());
    let mut payload = Vec::with_capacity(OUTPUT_CAP);
    payload.push(1);
    let output = reservation.output(payload).unwrap();
    assert!(reservation.output(vec![2]).is_err());
    let clone = output.clone();
    drop(output);
    drop(reservation);
    assert_eq!(admission.available_permits(), 0);
    drop(clone);
    assert_eq!(admission.available_permits(), 1);
}
#[test]
fn rejected_output_and_cancelled_exchange_release_admission() {
    let admission = Arc::new(Semaphore::new(1));
    let reservation = Reservation::acquire_from(&admission).unwrap();
    assert!(reservation.output(Vec::with_capacity(OUTPUT_CAP + 1)).is_err());
    drop(reservation);
    assert!(Reservation::acquire_from(&admission).is_some());
}
