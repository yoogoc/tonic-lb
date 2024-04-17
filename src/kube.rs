use std::collections::HashSet;

use futures::{future::join_all, prelude::*};
use k8s_openapi::api::core::v1::Endpoints;
use kube::{
    runtime::{
        watcher::{self, Event},
        WatchStreamExt,
    },
    Api, Client,
};
use tokio::sync::{mpsc::Sender, Mutex};
use tonic::transport::{Channel, Endpoint as TonicEndpoint, Uri};
use tower::discover::Change;

use crate::error::Error;

pub struct TargetInfo {
    service_name: String,
    service_namespace: String,
    port: String,
    resolve_by_port_name: bool,
    use_first_port: bool,
}

const KUBE_SCHEMA: &str = "kubernetes";

impl TryFrom<Uri> for TargetInfo {
    type Error = Error;

    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        if !matches!(uri.scheme_str(), Some(KUBE_SCHEMA) | None) {
            return Err(Error::NotMatchSchema(uri.scheme_str().unwrap().to_owned()));
        }

        todo!()
    }
}

#[allow(unused)]
pub async fn default_channel<T>(target: T) -> anyhow::Result<Channel>
where
    T: TryInto<TargetInfo, Error = Error>,
{
    channel(
        Client::try_default()
            .map_err(|err| Error::WrapKube(err))
            .await?,
        target,
    )
}

#[allow(unused)]
pub fn channel<T>(client: Client, target: T) -> anyhow::Result<Channel>
where
    T: TryInto<TargetInfo, Error = Error>,
{
    let (channel, sender) = Channel::balance_channel(1024);

    let target = target.try_into()?;
    tokio::spawn(watch(client, target, sender));
    Ok(channel)
}

async fn watch(
    client: Client,
    target: TargetInfo,
    sender: Sender<Change<String, TonicEndpoint>>,
) -> Result<(), Error> {
    let target = &target;
    let api = Api::<Endpoints>::namespaced(client, &target.service_namespace);
    let old_endpoints = Mutex::new(None);

    watcher::watcher(
        api,
        watcher::Config::default().fields(&format!("metadata.name={}", &target.service_name)),
    )
    .default_backoff()
    .map_err(|err| Error::WrapKubeWatcher(err))
    .try_for_each(|event| async {
        match event {
            Event::Applied(ref endpoints) => {
                let mut old_endpoints = old_endpoints.lock().await;
                let old_addresses = if let Some(old_endpoints) = old_endpoints.take() {
                    endpoints_to_addresses(&target, &old_endpoints)?
                } else {
                    HashSet::new()
                };
                let _ = old_endpoints.insert(endpoints.clone());
                drop(old_endpoints);

                let new_addresses = endpoints_to_addresses(&target, &endpoints)?;

                join_all(
                    old_addresses
                        .difference(&new_addresses)
                        .map(|address| sender.send(Change::Remove(address.to_owned())))
                        .chain(new_addresses.difference(&old_addresses).map(|address| {
                            sender.send(Change::Insert(
                                address.to_owned(),
                                TonicEndpoint::from_shared(address.to_owned()).unwrap(),
                            ))
                        })),
                )
                .await;
            }
            Event::Deleted(endpoints) => {
                let addresses = endpoints_to_addresses(&target, &endpoints)?;
                join_all(
                    addresses
                        .into_iter()
                        .map(|address| sender.send(Change::Remove(address))),
                )
                .await;
            }
            _ => {}
        }

        Ok(())
    })
    .await
}

fn endpoints_to_addresses(
    target: &TargetInfo,
    endpoints: &Endpoints,
) -> Result<HashSet<String>, Error> {
    let mut new_addresses = HashSet::new();
    if let Some(subsets) = &endpoints.subsets {
        for subset in subsets {
            if let Some(ports) = &subset.ports {
                let addresses = subset.addresses.clone().unwrap_or_default();
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
                    new_addresses.insert(host);
                }
            }
        }
    }
    Ok(new_addresses)
}
