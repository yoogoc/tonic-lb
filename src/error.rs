use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("kube: {0}")]
    WrapKube(kube::Error),
    #[error("kube watcher: {0}")]
    WrapKubeWatcher(kube::runtime::watcher::Error),
    #[error("not found endpoint port: {0}")]
    NotFoundPort(String),
    #[error("parse endpoint port error: {0}")]
    ParsePortError(String),
    #[error("not match schema: {0}")]
    NotMatchSchema(String),
}
