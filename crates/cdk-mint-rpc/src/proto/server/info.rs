//! Mint metadata service.

use cdk::nuts::MintInfo;
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::info::mint_info_service_server::MintInfoService;

impl MintRPCServer {
    /// Applies a mutation to the mint's info and returns the info in effect
    /// after the update
    async fn update_mint_info_with(
        &self,
        mutate: impl FnOnce(&mut MintInfo) + Send,
    ) -> Result<MintInfo, Status> {
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        mutate(&mut info);

        self.mint
            .set_mint_info(info.clone())
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(info)
    }
}

/// Converts an empty string to `None`, treating empty updates as clears
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Maps mint info contacts into their proto representation
fn info_contacts(contact: Option<Vec<cdk::nuts::ContactInfo>>) -> Vec<crate::info::ContactInfo> {
    contact
        .unwrap_or_default()
        .into_iter()
        .map(|c| crate::info::ContactInfo {
            method: c.method,
            info: c.info,
        })
        .collect()
}

#[tonic::async_trait]
impl MintInfoService for MintRPCServer {
    /// Returns the mint's public metadata
    async fn get_info(
        &self,
        _request: Request<crate::info::GetInfoRequest>,
    ) -> Result<Response<crate::info::GetInfoResponse>, Status> {
        let info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(Response::new(crate::info::GetInfoResponse {
            name: info.name,
            version: info.version.map(|v| v.to_string()),
            description: info.description,
            long_description: info.description_long,
            contact: info_contacts(info.contact),
            motd: info.motd,
            icon_url: info.icon_url,
            urls: info.urls.unwrap_or_default(),
            tos_url: info.tos_url,
        }))
    }

    /// Sets the mint's name
    async fn update_name(
        &self,
        request: Request<crate::info::UpdateNameRequest>,
    ) -> Result<Response<crate::info::UpdateNameResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let name = non_empty(request.into_inner().name);
        let info = self.update_mint_info_with(|info| info.name = name).await?;
        Ok(Response::new(crate::info::UpdateNameResponse {
            name: info.name,
        }))
    }

    /// Sets the mint's message of the day
    async fn update_motd(
        &self,
        request: Request<crate::info::UpdateMotdRequest>,
    ) -> Result<Response<crate::info::UpdateMotdResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let motd = non_empty(request.into_inner().motd);
        let info = self.update_mint_info_with(|info| info.motd = motd).await?;
        Ok(Response::new(crate::info::UpdateMotdResponse {
            motd: info.motd,
        }))
    }

    /// Sets the mint's short description
    async fn update_short_description(
        &self,
        request: Request<crate::info::UpdateShortDescriptionRequest>,
    ) -> Result<Response<crate::info::UpdateShortDescriptionResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let description = non_empty(request.into_inner().description);
        let info = self
            .update_mint_info_with(|info| info.description = description)
            .await?;
        Ok(Response::new(crate::info::UpdateShortDescriptionResponse {
            description: info.description,
        }))
    }

    /// Sets the mint's long description
    async fn update_long_description(
        &self,
        request: Request<crate::info::UpdateLongDescriptionRequest>,
    ) -> Result<Response<crate::info::UpdateLongDescriptionResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let long_description = non_empty(request.into_inner().long_description);
        let info = self
            .update_mint_info_with(|info| info.description_long = long_description)
            .await?;
        Ok(Response::new(crate::info::UpdateLongDescriptionResponse {
            long_description: info.description_long,
        }))
    }

    /// Sets the mint's icon URL
    async fn update_icon_url(
        &self,
        request: Request<crate::info::UpdateIconUrlRequest>,
    ) -> Result<Response<crate::info::UpdateIconUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let icon_url = non_empty(request.into_inner().icon_url);
        let info = self
            .update_mint_info_with(|info| info.icon_url = icon_url)
            .await?;
        Ok(Response::new(crate::info::UpdateIconUrlResponse {
            icon_url: info.icon_url,
        }))
    }

    /// Sets the mint's terms of service URL
    async fn update_tos_url(
        &self,
        request: Request<crate::info::UpdateTosUrlRequest>,
    ) -> Result<Response<crate::info::UpdateTosUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let tos_url = non_empty(request.into_inner().tos_url);
        let info = self
            .update_mint_info_with(|info| info.tos_url = tos_url)
            .await?;
        Ok(Response::new(crate::info::UpdateTosUrlResponse {
            tos_url: info.tos_url,
        }))
    }

    /// Adds a mint URL
    async fn add_url(
        &self,
        request: Request<crate::info::AddUrlRequest>,
    ) -> Result<Response<crate::info::AddUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let url = request.into_inner().url;
        if url.is_empty() {
            return Err(Status::invalid_argument(
                "URL must not be empty".to_string(),
            ));
        }
        let info = self
            .update_mint_info_with(|info| {
                let urls = info.urls.get_or_insert_with(Vec::new);
                if !urls.contains(&url) {
                    urls.push(url);
                }
            })
            .await?;
        Ok(Response::new(crate::info::AddUrlResponse {
            urls: info.urls.unwrap_or_default(),
        }))
    }

    /// Removes a mint URL
    async fn remove_url(
        &self,
        request: Request<crate::info::RemoveUrlRequest>,
    ) -> Result<Response<crate::info::RemoveUrlResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let url = request.into_inner().url;
        let info = self
            .update_mint_info_with(|info| {
                let mut urls = info.urls.take().unwrap_or_default();
                urls.retain(|u| u != &url);
                info.urls = if urls.is_empty() { None } else { Some(urls) };
            })
            .await?;
        Ok(Response::new(crate::info::RemoveUrlResponse {
            urls: info.urls.unwrap_or_default(),
        }))
    }

    /// Adds a contact entry
    async fn add_contact(
        &self,
        request: Request<crate::info::AddContactRequest>,
    ) -> Result<Response<crate::info::AddContactResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();
        if request.method.is_empty() {
            return Err(Status::invalid_argument(
                "Contact method must not be empty".to_string(),
            ));
        }
        if request.info.is_empty() {
            return Err(Status::invalid_argument(
                "Contact info must not be empty".to_string(),
            ));
        }
        let contact = cdk::nuts::ContactInfo::new(request.method, request.info);
        let info = self
            .update_mint_info_with(|info| {
                let contacts = info.contact.get_or_insert_with(Vec::new);
                if !contacts.contains(&contact) {
                    contacts.push(contact);
                }
            })
            .await?;
        Ok(Response::new(crate::info::AddContactResponse {
            contact: info_contacts(info.contact),
        }))
    }

    /// Removes a contact entry
    async fn remove_contact(
        &self,
        request: Request<crate::info::RemoveContactRequest>,
    ) -> Result<Response<crate::info::RemoveContactResponse>, Status> {
        self.ensure_mutation_allowed().await?;
        let request = request.into_inner();
        let contact = cdk::nuts::ContactInfo::new(request.method, request.info);
        let info = self
            .update_mint_info_with(|info| {
                if let Some(contacts) = info.contact.as_mut() {
                    contacts.retain(|c| c != &contact);
                }
                if info.contact.as_ref().is_some_and(Vec::is_empty) {
                    info.contact = None;
                }
            })
            .await?;
        Ok(Response::new(crate::info::RemoveContactResponse {
            contact: info_contacts(info.contact),
        }))
    }
}

#[cfg(test)]
mod tests {
    use tonic::Code;

    use super::super::test_utils::create_test_rpc_server;
    use super::*;

    #[tokio::test]
    async fn test_mint_info_service_get_info_round_trip() {
        let server = create_test_rpc_server().await;

        let response =
            MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
                .await
                .unwrap()
                .into_inner();

        assert_eq!(response.name.as_deref(), Some("test mint"));
        assert_eq!(response.description.as_deref(), Some("test mint"));
        assert!(response.tos_url.is_none());
        assert!(response.urls.is_empty());
        assert!(response.contact.is_empty());
    }

    #[tokio::test]
    async fn test_mint_info_service_update_motd_sets_and_clears() {
        let server = create_test_rpc_server().await;

        let set = MintInfoService::update_motd(
            &server,
            Request::new(crate::info::UpdateMotdRequest {
                motd: "hello".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(set.motd.as_deref(), Some("hello"));

        let cleared = MintInfoService::update_motd(
            &server,
            Request::new(crate::info::UpdateMotdRequest {
                motd: String::new(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(cleared.motd.is_none());

        let info = MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(info.motd.is_none());
    }

    #[tokio::test]
    async fn test_mint_info_service_update_tos_url() {
        let server = create_test_rpc_server().await;
        let tos = "https://example.com/terms";

        let response = MintInfoService::update_tos_url(
            &server,
            Request::new(crate::info::UpdateTosUrlRequest {
                tos_url: tos.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(response.tos_url.as_deref(), Some(tos));

        let info = MintInfoService::get_info(&server, Request::new(crate::info::GetInfoRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(info.tos_url.as_deref(), Some(tos));
    }

    #[tokio::test]
    async fn test_mint_info_service_add_url_is_idempotent() {
        let server = create_test_rpc_server().await;
        let url = "https://mint.example.com";

        let first = MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(first.urls, vec![url.to_owned()]);

        let second = MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(second.urls, vec![url.to_owned()]);
    }

    #[tokio::test]
    async fn test_mint_info_service_add_url_rejects_empty() {
        let server = create_test_rpc_server().await;

        let error = MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest { url: String::new() }),
        )
        .await
        .expect_err("empty URL should be rejected");

        assert_eq!(error.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_mint_info_service_remove_url_absent_is_noop() {
        let server = create_test_rpc_server().await;
        let url = "https://mint.example.com";

        MintInfoService::add_url(
            &server,
            Request::new(crate::info::AddUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap();

        let missing = MintInfoService::remove_url(
            &server,
            Request::new(crate::info::RemoveUrlRequest {
                url: "https://absent.example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(missing.urls, vec![url.to_owned()]);

        let removed = MintInfoService::remove_url(
            &server,
            Request::new(crate::info::RemoveUrlRequest {
                url: url.to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(removed.urls.is_empty());
    }

    #[tokio::test]
    async fn test_mint_info_service_contacts_add_remove_idempotent() {
        let server = create_test_rpc_server().await;

        let added = MintInfoService::add_contact(
            &server,
            Request::new(crate::info::AddContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(added.contact.len(), 1);

        let duplicate = MintInfoService::add_contact(
            &server,
            Request::new(crate::info::AddContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(duplicate.contact.len(), 1);

        let empty_method_error = MintInfoService::add_contact(
            &server,
            Request::new(crate::info::AddContactRequest {
                method: String::new(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .expect_err("empty contact method should be rejected");
        assert_eq!(empty_method_error.code(), Code::InvalidArgument);

        let removed = MintInfoService::remove_contact(
            &server,
            Request::new(crate::info::RemoveContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(removed.contact.is_empty());

        let absent = MintInfoService::remove_contact(
            &server,
            Request::new(crate::info::RemoveContactRequest {
                method: "email".to_owned(),
                info: "mint@example.com".to_owned(),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(absent.contact.is_empty());
    }
}
