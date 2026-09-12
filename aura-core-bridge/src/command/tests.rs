#[cfg(test)]
mod tests {
    use super::*;

include!("tests/analysis_and_validation.rs");
include!("tests/mutations_and_history.rs");
include!("tests/integration_contracts.rs");
}
