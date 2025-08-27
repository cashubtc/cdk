#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
#[allow(dead_code)]
pub(crate) mod lnrpc {
    tonic::include_proto!("lnrpc");
}

#[allow(clippy::all, clippy::pedantic, clippy::restriction, clippy::nursery)]
pub(crate) mod routerrpc {
    tonic::include_proto!("routerrpc");
}
