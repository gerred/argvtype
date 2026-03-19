use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LspError {
    #[error("LSP server is not yet implemented")]
    NotImplemented,
}

pub fn run_server() -> Result<(), LspError> {
    Err(LspError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_server_returns_not_implemented() {
        let result = run_server();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LspError::NotImplemented));
    }
}
