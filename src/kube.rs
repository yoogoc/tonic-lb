use futures::prelude::*;
use k8s_openapi::api::core::v1::Endpoints;
use kube::{
    runtime::{watcher, WatchStreamExt},
    Api, Client,
};
use tokio::sync::mpsc::Sender;
use tonic::transport::{Channel, Endpoint as TonicEndpoint, Uri};
use tower::discover::Change;

use crate::error::Error;

struct TargetInfo {
    _service_name: String,
    _service_namespace: String,
    port: String,
    resolve_by_port_name: bool,
    use_first_port: bool,
}

impl TryFrom<Uri> for TargetInfo {
    type Error = Error;

    fn try_from(_value: Uri) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[allow(unused)]
pub fn balance_channel(target: Uri) -> anyhow::Result<Channel> {
    let (channel, sender) = Channel::balance_channel(1024);

    let target = target.try_into()?;
    tokio::spawn(watch(target, sender));
    Ok(channel)
}

async fn watch(
    target: TargetInfo,
    sender: Sender<Change<String, TonicEndpoint>>,
) -> Result<(), Error> {
    let client = Client::try_default()
        .await
        .map_err(|err| Error::WrapKube(err))?;
    let api = Api::<Endpoints>::namespaced(client, "TODO");
    let target = &target;

    let name = "TODO";
    watcher::watcher(
        api,
        watcher::Config::default().fields(&format!("metadata.name={name}")),
    )
    .applied_objects()
    .default_backoff()
    .map_err(|err| Error::WrapKubeWatcher(err))
    .try_for_each(|endpoints| async {
        if let Some(subsets) = endpoints.subsets {
            for subset in subsets {
                if let Some(ports) = subset.ports {
                    let addresses = subset.addresses.unwrap_or_default();
                    let port = if target.use_first_port {
                        Ok(ports[0].port)
                    } else if target.resolve_by_port_name {
                        let port = ports
                            .iter()
                            .filter(|p| p.name.as_ref().is_some_and(|name| name.eq(&target.port)))
                            .next()
                            .ok_or(Error::NotFoundPort(target.port.to_string()));
                        if let Ok(port) = port {
                            Ok(port.port)
                        } else {
                            Err(Error::NotFoundPort((&target.port).to_owned()))
                        }
                    } else {
                        target
                            .port
                            .parse::<i32>()
                            .map_err(|_err| Error::ParsePortError((&target.port).to_owned()))
                    }?;
                    for address in addresses {
                        let host = format!("{}:{}", address.ip, port);
                        sender
                            .send(Change::Insert(
                                host.clone(),
                                TonicEndpoint::from_shared(host).unwrap(),
                            ))
                            .await
                            .unwrap();
                    }
                }
            }
        }

        Ok(())
    })
    .await
}
