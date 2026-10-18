use super::ConfigEvalError;
use zfb_js_host::native::NativeError;

pub async fn evaluate_config_bundle(source: &str) -> Result<serde_json::Value, ConfigEvalError> {
    zfb_js_host::native::evaluate_config_bundle(source)
        .await
        .map_err(|error| match error {
            NativeError::NotAPlainObject => ConfigEvalError::NotAPlainObject,
            NativeError::BareImportRejected(message) => {
                ConfigEvalError::BareImportRejected(message)
            }
            NativeError::ModuleLoad(message) => ConfigEvalError::ModuleLoad(message),
            NativeError::Evaluation(message) | NativeError::Bootstrap(message) => {
                ConfigEvalError::Evaluation(message)
            }
        })
}
