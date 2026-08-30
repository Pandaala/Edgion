// Transport-shape coverage for the request-body seam.
//
// See `transport.rs` for the suite itself. The split exists so the module has a
// place to grow response-side shapes when the response streaming interface
// lands (deliberately out of scope today — there is nothing to assert yet).

mod transport;

pub use transport::TransportTestSuite;
