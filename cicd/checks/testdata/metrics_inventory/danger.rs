fn emit() {
    metrics::counter!("edgion_bad_total", "path" => p).increment(1);
    metrics::counter!("edgion_ok_total", "status" => s).increment(1);
}
