#![doc = include_str!("../README.md")]

pub mod proto;

mod wallet_info;

pub mod mint_rpc_cli;

pub use proto::*;
pub use wallet_info::{
    DynWalletInfoProvider, WalletAddressPage, WalletInfoError, WalletInfoProvider,
    WalletTransactionPage,
};

/// Type alias for MintInfoServiceClient with the version header interceptor over a Channel
pub type InterceptedMintInfoServiceClient = info::mint_info_service_client::MintInfoServiceClient<
    tonic::codegen::InterceptedService<
        tonic::transport::Channel,
        cdk_common::grpc::VersionInterceptor,
    >,
>;

/// Type alias for KeysetServiceClient with the version header interceptor over a Channel
pub type InterceptedKeysetServiceClient = keyset::keyset_service_client::KeysetServiceClient<
    tonic::codegen::InterceptedService<
        tonic::transport::Channel,
        cdk_common::grpc::VersionInterceptor,
    >,
>;

/// Type alias for PaymentMethodServiceClient with the version header interceptor over a Channel
pub type InterceptedPaymentMethodServiceClient =
    payment_method::payment_method_service_client::PaymentMethodServiceClient<
        tonic::codegen::InterceptedService<
            tonic::transport::Channel,
            cdk_common::grpc::VersionInterceptor,
        >,
    >;

/// Type alias for QuoteServiceClient with the version header interceptor over a Channel
pub type InterceptedQuoteServiceClient = quote::quote_service_client::QuoteServiceClient<
    tonic::codegen::InterceptedService<
        tonic::transport::Channel,
        cdk_common::grpc::VersionInterceptor,
    >,
>;

/// Type alias for WalletServiceClient with the version header interceptor over a Channel
pub type InterceptedWalletServiceClient = wallet::wallet_service_client::WalletServiceClient<
    tonic::codegen::InterceptedService<
        tonic::transport::Channel,
        cdk_common::grpc::VersionInterceptor,
    >,
>;
