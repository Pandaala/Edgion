pub mod names {
    pub const A_TOTAL: &str = "edgion_sample_a_total";
}
const B_GAUGE: &str = "edgion_sample_b";
fn emit() {
    metrics::counter!(names::A_TOTAL).increment(1);
    metrics::gauge!("edgion_sample_c").set(1.0);
    out.push_str(B_GAUGE);
}
