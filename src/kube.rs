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
    service_namespace: Option<String>,
    port: Option<String>,
}

const KUBE_SCHEMA: &str = "kubernetes";

impl TryFrom<Uri> for TargetInfo {
    type Error = Error;

    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        if !matches!(uri.scheme_str(), Some(KUBE_SCHEMA) | None) {
            return Err(Error::NotMatchSchema(uri.scheme_str().unwrap().to_owned()));
        }
        let host = uri.host().ok_or(Error::HostIsEmpty)?;
        let (service, port) = if let Some(i) = host.chars().position(|c| c == ':') {
            let port = &host[i + 1..];
            (&host[..i], Some(port.to_string()))
        } else {
            (host, None)
        };

        let parts: Vec<_> = service.split(".").collect();
        if parts.len() == 1 {
            Ok(Self {
                service_name: host.to_string(),
                service_namespace: None,
                port,
            })
        } else if parts.len() >= 2 {
            Ok(Self {
                service_name: (&parts[0]).to_string(),
                service_namespace: Some((&parts[1]).to_string()),
                port,
            })
        } else {
            unreachable!()
        }
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
    let api = if let Some(service_namespace) = &target.service_namespace {
        Api::<Endpoints>::namespaced(client, service_namespace)
    } else {
        Api::<Endpoints>::default_namespaced(client)
    };

    let endpoints = api
        .get(&target.service_name)
        .await
        .map_err(|err| Error::WrapKubeClient(err.to_string()))?;
    let new_addresses = endpoints_to_addresses(&target, &endpoints)?;
    join_all(new_addresses.into_iter().map(|address| {
        sender.send(Change::Insert(
            address.to_owned(),
            TonicEndpoint::from_shared(address.to_owned()).unwrap(),
        ))
    }))
    .await;

    let old_endpoints = Mutex::new(endpoints);

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
                let old_addresses = endpoints_to_addresses(&target, &old_endpoints)?;
                *old_endpoints = endpoints.clone();
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
                let port = match &target.port {
                    Some(port) => {
                        if let Ok(port) = port.parse::<i32>() {
                            Ok(port)
                        } else {
                            Ok(ports
                                .iter()
                                .filter(|p| p.name.as_ref().is_some_and(|name| name.eq(port)))
                                .next()
                                .ok_or(Error::NotFoundPort(port.to_string()))?
                                .port)
                        }
                    }
                    None => Ok(ports[0].port),
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
