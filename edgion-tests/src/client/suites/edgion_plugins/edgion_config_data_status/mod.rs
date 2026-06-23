pub mod dynamic;
mod edgion_config_data_status;
pub use dynamic::{
    EdgionConfigDataFlapInitialTestSuite, EdgionConfigDataFlapInvalidTestSuite, EdgionConfigDataFlapValidTestSuite,
};
pub use edgion_config_data_status::EdgionConfigDataStatusTestSuite;
