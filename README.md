# tonic-lb

A Grpc name resolver for [`tonic`](https://github.com/hyperium/tonic)

## usage

```rust
let uri = "kubernetes://service-name:8080/".into();
let channel = tonic_lb::kube::default_channel(uri).await?;
let client = YourServiceClient::new(channel);
```

## an url can be one of the following

```plain
kubernetes://service-name:8080/
kubernetes://service-name.namespace:8080/
kubernetes://service-name.namespace.svc.cluster_name
kubernetes://service-name.namespace.svc.cluster_name:8080

service-name:8080/
service-name.namespace:8080/
service-name.namespace.svc.cluster_name
service-name.namespace.svc.cluster_name:8080
```
