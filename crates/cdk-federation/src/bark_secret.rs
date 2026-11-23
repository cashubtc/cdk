//! Authenticated private transport for protected Bark receive secrets.

use cdk::federation::{
    FederationBarkPrepareReceiveSecretRequest, FederationBarkReceiveSecret,
    FederationBarkReceiveSecretAcknowledgement, FederationBarkReceiveSecretBinding,
    FederationConfig, MemberConfigPublic,
};
use futures::future::join_all;

/// Bounded collector for one exact protected receive-secret replication set.
#[derive(Debug)]
pub struct FederationBarkReceiveSecretCoordinator<'a, T> {
    client: &'a super::peer_client::FederationRestPeerClient<T>,
}

impl<'a, T> FederationBarkReceiveSecretCoordinator<'a, T>
where
    T: cdk_http_client::Transport,
{
    pub fn new(client: &'a super::peer_client::FederationRestPeerClient<T>) -> Self {
        Self { client }
    }

    /// Replicate an exact preimage to a sorted eligible set and require a
    /// signature threshold including the designated invoice actor.
    pub async fn distribute(
        &self,
        config: &FederationConfig,
        binding: &FederationBarkReceiveSecretBinding,
        secret: &FederationBarkReceiveSecret,
        eligible: &[MemberConfigPublic],
        initial: Vec<FederationBarkReceiveSecretAcknowledgement>,
    ) -> anyhow::Result<Vec<FederationBarkReceiveSecretAcknowledgement>> {
        binding.validate(config)?;
        let mut previous = None;
        for member in eligible {
            if previous.is_some_and(|previous| previous >= member.member_id)
                || config.member(member.member_id) != Some(member)
            {
                anyhow::bail!("Bark receive-secret recipients are not a sorted configured set");
            }
            previous = Some(member.member_id);
        }
        let mut acknowledgements = initial;
        let results = join_all(eligible.iter().map(|member| {
            let request = FederationBarkPrepareReceiveSecretRequest::new(
                binding.clone(),
                member.member_id,
                *secret.as_bytes(),
            );
            async move {
                (
                    member.member_id,
                    self.client
                        .prepare_bark_receive_secret(member, &request)
                        .await,
                )
            }
        }))
        .await;
        for (member, result) in results {
            match result {
                Ok(acknowledgement) => acknowledgements.push(acknowledgement),
                Err(error) => {
                    tracing::warn!(
                        member = %member,
                        error = %error,
                        "Bark receive-secret replication failed"
                    );
                }
            }
        }
        acknowledgements.sort_by_key(|acknowledgement| acknowledgement.member);
        if acknowledgements
            .windows(2)
            .any(|pair| pair[0].member >= pair[1].member)
        {
            anyhow::bail!("Bark receive-secret acknowledgements are not unique");
        }
        for acknowledgement in &acknowledgements {
            acknowledgement.verify(config, binding)?;
        }
        if acknowledgements.len() < usize::from(config.threshold_params.signature_threshold)
            || !acknowledgements
                .iter()
                .any(|acknowledgement| acknowledgement.member == binding.actor)
        {
            anyhow::bail!("Bark receive-secret acknowledgement threshold was not reached");
        }
        Ok(acknowledgements)
    }
}
