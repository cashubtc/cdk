use std::time::Duration;
use std::vec;

use crate::sub_commands::dlc::UserBet;
use nostr_sdk::base64::prelude::*;
use nostr_sdk::event::builder::Error;
use nostr_sdk::nips::nip04;
use nostr_sdk::{
    Client, Event, EventBuilder, EventId, EventSource, Filter, Keys, Kind, PublicKey, Tag,
};

/// Create Kind 8_888 event tagged with the counterparty pubkey
///
/// see https://github.com/nostr-protocol/nips/blob/9157321a224bca77b3472a19de72885af9d6a91d/88.md#kind8_888
///
/// # Arguments
/// * `keys` - The Keys used to sign the event
/// * `msg` - The dlc message
/// * `counterparty_pubkey` - Public key to send this message to
pub fn create_dlc_msg_event(
    keys: &Keys,
    msg: String,
    counterparty_pubkey: &PublicKey,
) -> Result<Event, Error> {
    // The DLC message is first serialized in binary, and then encrypted with NIP04.
    let content = BASE64_STANDARD.encode(msg);

    let content: String = nip04::encrypt(&keys.secret_key(), counterparty_pubkey, content)?;

    EventBuilder::new(
        Kind::Custom(8888),
        content,
        vec![Tag::public_key(*counterparty_pubkey)],
    )
    .to_event(keys)
}

pub async fn lookup_announcement_event(
    event_id: EventId,
    client: &Client,
) -> Option<Result<Event, Error>> {
    let filter = Filter::new().id(event_id).kind(Kind::Custom(88));
    let events = client
        .get_events_of(
            vec![filter],
            EventSource::both(Some(Duration::from_secs(10))),
        )
        .await
        .expect("get_events_of failed");
    if events.is_empty() {
        return None;
    }
    Some(Ok(events.first().unwrap().clone()))
}

pub async fn list_dlc_offers(
    keys: &Keys,
    client: &Client,
    event_id: Option<EventId>,
) -> Option<Vec<UserBet>> {
    let filter = if let Some(event_id) = event_id {
        Filter::new().id(event_id)
    } else {
        Filter::new()
            .kind(Kind::Custom(8888))
            .pubkey(keys.public_key())
    };
    let events = client
        .get_events_of(
            vec![filter],
            EventSource::both(Some(Duration::from_secs(10))),
        )
        .await
        .expect("get_events_of failed");

    if events.is_empty() {
        return None;
    }

    let offers = events
        .iter()
        .map(|e| {
            let decrypted =
                nostr_sdk::nips::nip04::decrypt(keys.secret_key(), &e.pubkey, e.content.clone())
                    .unwrap();

            let decoded = BASE64_STANDARD.decode(&decrypted).unwrap();
            let decoded_str = std::str::from_utf8(&decoded).unwrap();
            serde_json::from_str::<UserBet>(decoded_str).unwrap()
        })
        .collect();
    Some(offers)
}

/// Used to reset the state of our offers on the relays in case we change types of UserBet
pub async fn delete_all_dlc_offers(keys: &Keys, client: &Client) -> Option<Vec<EventId>> {
    let filter = Filter::new()
        .kind(Kind::Custom(8888))
        .author(keys.public_key());
    let events = client
        .get_events_of(
            vec![filter],
            EventSource::both(Some(Duration::from_secs(10))),
        )
        .await
        .expect("get_events_of failed");

    if events.is_empty() {
        return None;
    }

    let mut deleted: Vec<EventId> = Vec::new();

    for event in events {
        let out = client.delete_event(event.id).await.unwrap();
        if out.success.len() > 0 {
            deleted.push(event.id);
        }
    }
    Some(deleted)
}

/// Lookup the attestation event for a given announcement event id
pub async fn lookup_attestation_event(
    announcement_event_id: EventId,
    client: &Client,
) -> Option<Result<Event, Error>> {
    let filter = Filter::new()
        .event(announcement_event_id)
        .kind(Kind::Custom(89));
    let events = client
        .get_events_of(
            vec![filter],
            EventSource::both(Some(Duration::from_secs(10))),
        )
        .await
        .unwrap();

    if events.len() != 1 {
        return None;
    }

    Some(Ok(events.first().unwrap().clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::{Client, EventId, Keys};

    #[tokio::test]
    async fn test_lookup_announcement_event() {
        let announcement_id =
            EventId::from_hex("d30e6c857a900ebefbf7dc3b678ead9215f4345476067e146ded973971286529")
                .unwrap();

        let client = Client::new(&Keys::generate());
        //let relay = "wss://relay.damus.io";
        let relay = "relay.nostrdice.com";
        client.add_relay(relay.to_string()).await.unwrap();
        client.connect().await;
        let event = lookup_announcement_event(announcement_id, &client)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.id, announcement_id);
    }

    #[test]
    fn test_create_dlc_message_event() {
        let keys = Keys::parse("4e111131d31ad92ed5d37ab87d5046efa730f192f9c8f9b59f6c61caad1f8933")
            .unwrap();
        let counterparty_pubkey = Keys::generate().public_key();
        let msg = String::from("hello");

        let msg = BASE64_STANDARD.encode(msg);

        let event = create_dlc_msg_event(&keys, msg.clone(), &counterparty_pubkey).unwrap();

        assert_eq!(keys.public_key(), event.pubkey);
        assert_eq!(Kind::Custom(8888), event.kind);
        println!("{:?}", event)
    }

    #[tokio::test]
    async fn test_list_dlc_offers() {
        let keys = Keys::generate();
        let counterparty_privkey = Keys::generate();
        let counterparty_pubkey = counterparty_privkey.public_key();
        let msg = String::from("hello");
        let msg = BASE64_STANDARD.encode(msg);

        let event = create_dlc_msg_event(&keys, msg.clone(), &counterparty_pubkey).unwrap();

        let client = Client::new(&Keys::generate());
        //let relay = "wss://relay.damus.io";
        let relay = "relay.nostrdice.com";
        client.add_relay(relay.to_string()).await.unwrap();
        client.connect().await;

        let event_id = client.send_event(event).await.unwrap();

        println!("event id: {:?}", event_id.to_hex());

        let offers = list_dlc_offers(&counterparty_privkey, &client, None)
            .await
            .unwrap(); // error in line 74:58

        assert!(offers.len() >= 1);

        /* clean up */
        delete_all_dlc_offers(&keys, &client).await;
    }

    #[test]
    fn test_deserialize_from_string() {
        let str ="{\"id\":7,\"oracle_announcement\":{\"announcementSignature\":\"ca9cb2c97ea975950cf8ea2f1dfb712bd4ad22677c17b9f19abfde4dec91ff992839555efc468dc353007899bc2ca0d5d70ef816bf9427ef376d53f7ca73668f\",\"oraclePublicKey\":\"029bb49afd78fe627b613199ea1dee4c38b8db2e5dcc3ab6a245d455c7ddf4c5\",\"oracleEvent\":{\"oracleNonces\":[\"c8438011df79ef5e432ab48b55d37fedc1ad3a54914d0d3f2ec5bfa7706254e7\"],\"eventMaturityEpoch\":1705363200,\"eventDescriptor\":{\"enumEvent\":{\"outcomes\":[\"1234\",\"4567\"]}},\"eventId\":\"test\"}},\"oracle_event_id\":\"d30e6c857a900ebefbf7dc3b678ead9215f4345476067e146ded973971286529\",\"user_outcomes\":[\"1234\",\"4567\"],\"blinding_factor\":\"54333ffa98687d4e7dc46e480deb6c4093ce6fe9a9bfef8a1f5e6950d25e1c14\",\"dlc_root\":\"96e0a0737aaae1a83e389300ffea9eb9a571038719d6ff2fb25fb40144998bf2\",\"timeout\":1705366800}";
        let bet = serde_json::from_str::<UserBet>(str).unwrap();

        println!("{:?}", bet);
    }
}
