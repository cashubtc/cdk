//! One request per selected FROST signer, answered by the local signer runtime
//! for this member and over authenticated REST for every other member.
//!
//! The FROST signing coordinator and the Bark custody coordinator both run
//! their rounds through [`fan_out`], so concurrency, ordering and error
//! handling are the same for every custody protocol.

use std::future::Future;

use cdk::federation::{FederationConfig, FederationError, MemberConfigPublic, MemberId};
use cdk::mint::Mint;
use futures::future::Either;
use futures::stream::{self, StreamExt};

use super::frost_signing::FederationFrostSigningRuntime;
use super::peer_client::FederationRestPeerClient;

/// This member's signer runtime, used instead of a REST round trip to itself.
///
/// The local path runs the same authorization validator and durable nonce
/// store as the REST handlers.
#[derive(Debug, Clone, Copy)]
pub struct LocalFrostSigner<'a> {
    /// Local member identifier.
    pub member_id: MemberId,
    /// Local mint whose database holds the signer's durable state.
    pub mint: &'a Mint,
    /// Local signer runtime.
    pub runtime: &'a FederationFrostSigningRuntime,
}

/// Resolve every selected signer to its member configuration.
pub fn signer_members<'c>(
    public_config: &'c FederationConfig,
    signers: &[cdk_frost::MemberId],
) -> anyhow::Result<Vec<(cdk_frost::MemberId, &'c MemberConfigPublic)>> {
    signers
        .iter()
        .map(|signer| {
            let member_id = MemberId::new(signer.get())?;
            let member = public_config
                .member(member_id)
                .ok_or(FederationError::UnknownMember { member_id })?;
            Ok((*signer, member))
        })
        .collect()
}

/// The selected signers that resolve to configured members, skipping any that
/// do not. Used for best-effort notifications such as aborts.
pub fn known_signer_members<'c>(
    public_config: &'c FederationConfig,
    signers: &[cdk_frost::MemberId],
) -> Vec<&'c MemberConfigPublic> {
    signers
        .iter()
        .filter_map(|signer| MemberId::new(signer.get()).ok())
        .filter_map(|member_id| public_config.member(member_id))
        .collect()
}

/// Concurrent requests allowed for one round over `signers` members.
pub fn concurrency_limit<T>(client: &FederationRestPeerClient<T>, signers: usize) -> usize {
    signers
        .min(client.sync_config.private_rest_outbound_concurrency_limit)
        .max(1)
}

/// Send one request to each member: `on_local` for this member, `on_remote`
/// for the others, at most `limit` at a time.
///
/// Every request resolves before this returns, so a caller that aborts the
/// session afterwards never races a request still in flight. Results keep the
/// members' order, and the first error in that order is returned.
pub async fn fan_out<'a, 'c, X, T, L, LF, R, RF>(
    members: Vec<(&'c MemberConfigPublic, X)>,
    local: Option<LocalFrostSigner<'a>>,
    limit: usize,
    on_local: L,
    on_remote: R,
) -> anyhow::Result<Vec<T>>
where
    L: Fn(LocalFrostSigner<'a>, X) -> LF,
    LF: Future<Output = anyhow::Result<T>>,
    R: Fn(&'c MemberConfigPublic, X) -> RF,
    RF: Future<Output = anyhow::Result<T>>,
{
    // Build the futures before streaming them: futures are lazy, so `buffered`
    // still bounds concurrency, and no closure is held across an await.
    let requests = members
        .into_iter()
        .map(
            |(member, input)| match local.filter(|local| local.member_id == member.member_id) {
                Some(local) => Either::Left(on_local(local, input)),
                None => Either::Right(on_remote(member, input)),
            },
        )
        .collect::<Vec<_>>();
    stream::iter(requests)
        .buffered(limit.max(1))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}
