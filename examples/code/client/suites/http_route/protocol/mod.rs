// HTTP Route Protocol test module
// Note: HTTPS moved to EdgionTls module

mod h2_trailer_malformed;
mod h2c;
mod websocket;

pub use h2_trailer_malformed::H2TrailerMalformedTestSuite;
pub use h2c::H2cTestSuite;
pub use websocket::WebSocketTestSuite;
