//! Sealed blobs: a signed, encrypted payload addressed to one contact. Relays only ever see this
//! (`connectivity/seal.ts`).
//!
//! ECDH (fresh ephemeral key) -> HKDF-SHA-256(salt `duochat-seal-v1`, info `to|from`) -> AES-256-GCM (aad `to|from`),
//! then ECDSA over the canonical JSON of everything but the signature.

use crate::{
    aead,
    canonical::canonical_json,
    devices::{AccountKeys, Author, DeviceList, Signer, LIST_FIELD},
    error::CryptoError,
    identity::{jwk_of, public_key_from_jwk, thumb, verify_signature, Identity},
    kdf::{hkdf_sha256, sha256_hex},
    rng::{random_array, random_secret_key, Entropy},
};
use p256::ecdh::diffie_hellman;
use privcord_domain::{
    blob::{PublicJwk, SealedBlob, MAX_SEALED_BLOB_BYTES},
    codec,
    ids::UserId,
};
use serde_json::{json, Value};

fn seal_salt() -> Vec<u8> {
    crate::kdf::label(&["duochat", "seal", "v1"])
}
const MAILBOX_PREFIX: &str = "duochat-mailbox-v1|";

/// The bytes a blob's signature covers.
pub fn signed_part(blob: &SealedBlob) -> Result<Vec<u8>, CryptoError> {
    let mut v = json!({ "v": blob.v, "to": blob.to, "from": blob.from, "epk": thumb(&blob.epk), "iv": blob.iv, "ct": blob.ct });
    // v2 also signs the ML-KEM ciphertext and which key it was sealed to, so neither can be swapped in transit.
    if let (Some(kem), Some(key), Some(o)) = (&blob.kem, &blob.key, v.as_object_mut()) {
        o.insert("kem".into(), json!(kem));
        o.insert("key".into(), json!(key));
    }
    Ok(canonical_json(&v)?.into_bytes())
}

/// What a blob is remembered by, to drop copies already handled. Not the signature string: the same signature has
/// several encodings that all verify (spare base64 bits, high or low S), so a replayer could re-send each as "new".
/// The signed content has only one.
pub fn replay_id(blob: &SealedBlob) -> String {
    signed_part(blob).map_or_else(|_| blob.sig.clone(), |b| sha256_hex(&b))
}

/// The name of a private inbox made from a secret only its owner holds (see `Identity::derive_secret`). Unlike
/// [`mailbox_for`] it cannot be worked out from a public id, so only people the owner told can find it.
pub fn inbox_mailbox(secret: &[u8]) -> String {
    let mut input = b"duocord-inbox-v1|".to_vec();
    input.extend_from_slice(secret);
    sha256_hex(&input)
}

/// Where a client publishes to and listens on. Hashing keeps the uid out of relay logs.
pub fn mailbox_for(uid: &str) -> String {
    sha256_hex(format!("{MAILBOX_PREFIX}{uid}").as_bytes())
}

/// Seal `payload` for one contact, signed by `identity` (the account key).
pub fn seal(
    identity: &Identity,
    to: &UserId,
    to_dh: &PublicJwk,
    payload: &Value,
    entropy: &mut dyn Entropy,
) -> Result<SealedBlob, CryptoError> {
    let uid = identity.uid()?;
    seal_as(
        &Author::account(&uid, identity),
        to,
        to_dh,
        payload,
        entropy,
    )
}

/// Seal `payload` for one contact, from the account, signed by `author` (the account key or one of its devices).
pub fn seal_as(
    author: &Author<'_>,
    to: &UserId,
    to_dh: &PublicJwk,
    payload: &Value,
    entropy: &mut dyn Entropy,
) -> Result<SealedBlob, CryptoError> {
    let from = author.uid;
    let ephemeral = random_secret_key(entropy)?;
    let peer = public_key_from_jwk(to_dh)?;
    let shared = diffie_hellman(ephemeral.to_nonzero_scalar(), peer.as_affine());
    let key = hkdf_sha256::<32>(
        shared.raw_secret_bytes(),
        &seal_salt(),
        format!("{to}|{from}").as_bytes(),
    )?;
    let iv: [u8; 12] = random_array(entropy)?;
    let plaintext =
        serde_json::to_vec(&author.payload(payload)).map_err(|_| CryptoError::Invalid)?;
    let ct = aead::encrypt(&key, &iv, &plaintext, format!("{to}|{from}").as_bytes())?;
    let mut blob = SealedBlob {
        v: 1,
        to: to.to_string(),
        from: from.to_string(),
        epk: jwk_of(&ephemeral.public_key()),
        iv: codec::encode(&iv),
        ct: codec::encode(&ct),
        sig: String::new(),
        kem: None,
        key: None,
    };
    blob.sig = codec::encode(&author.sign(&signed_part(&blob)?));
    if blob.encode().encode_utf16().count() > MAX_SEALED_BLOB_BYTES {
        return Err(CryptoError::TooLarge);
    }
    Ok(blob)
}

/// Signs a blob that is not ECDH-sealed (invite and channel blobs keep the same shape).
pub fn sign_blob(identity: &Identity, unsigned: &SealedBlob) -> Result<String, CryptoError> {
    Ok(codec::encode(
        &identity.sign_message(&signed_part(unsigned)?),
    ))
}

pub fn verify_blob_signature(blob: &SealedBlob, sign_pub: &PublicJwk) -> bool {
    let (Some(sig), Ok(part)) = (codec::decode(&blob.sig), signed_part(blob)) else {
        return false;
    };
    verify_signature(sign_pub, &sig, &part)
}

/// A blob opened: what it said, who of the account signed it, and the device list it carried (for the caller to
/// [`AccountKeys::learn`]; [`LIST_FIELD`] is taken out of the payload).
#[derive(Debug, Clone)]
pub struct Opened {
    pub payload: Value,
    pub signer: Signer,
    pub list: Option<DeviceList>,
}

/// Checks who of `keys`' account signed `blob`, given its decrypted payload, and takes the carried list out of it.
pub(crate) fn signed_by(
    blob: &SealedBlob,
    keys: &AccountKeys,
    mut payload: Value,
) -> Option<Opened> {
    let list = AccountKeys::carried(&payload);
    let signer = keys.signer(
        &codec::decode(&blob.sig)?,
        &signed_part(blob).ok()?,
        list.as_ref(),
    )?;
    if let Some(o) = payload.as_object_mut() {
        o.remove(LIST_FIELD);
    }
    Some(Opened {
        payload,
        signer,
        list,
    })
}

fn decrypt(blob: &SealedBlob, own: &Identity) -> Option<Value> {
    let shared = own.agree(&blob.epk).ok()?;
    let aad = format!("{}|{}", blob.to, blob.from);
    let key = hkdf_sha256::<32>(&*shared, &seal_salt(), aad.as_bytes()).ok()?;
    let plain = aead::decrypt(
        &key,
        &codec::decode(&blob.iv)?,
        &codec::decode(&blob.ct)?,
        aad.as_bytes(),
    )
    .ok()?;
    serde_json::from_slice(&plain).ok()
}

/// The payload, or `None` for anything not addressed to us, not signed for the pinned sender (by their account key or
/// one of their devices), or changed in transit.
pub fn open(
    blob: &SealedBlob,
    own_uid: &UserId,
    own: &Identity,
    from: &AccountKeys,
) -> Option<Opened> {
    if blob.v != 1 || blob.to != own_uid.as_str() || blob.from != from.uid.as_str() {
        return None;
    }
    signed_by(blob, from, decrypt(blob, own)?)
}

/// Open a blob from someone who is not a contact yet: a friend request after they added us from our card (a QR code).
/// The payload must carry their contact card under `"card"`. Their identity is taken from that card (its proof binds the
/// signing key to the key the uid comes from) and must be the blob's sender; only then is the signature checked with the
/// card's own key (or a device on the account list the payload carries). So a request can only come from whoever holds
/// that identity's private keys, or a device they listed.
pub fn open_from_stranger(
    blob: &SealedBlob,
    own_uid: &UserId,
    own: &Identity,
) -> Option<(crate::card::Contact, Opened)> {
    if blob.v != 1 || blob.to != own_uid.as_str() {
        return None;
    }
    let payload = decrypt(blob, own)?;
    let contact = crate::card::ContactCard::decode(payload.get("card")?.as_str()?)?;
    if contact.uid.as_str() != blob.from {
        return None;
    }
    let keys = AccountKeys::new(
        contact.uid.clone(),
        contact.card.sign.clone(),
        contact.card.dh.clone(),
        None,
    );
    let opened = signed_by(blob, &keys, payload)?;
    Some((contact, opened))
}

/// A fresh throwaway key whose public half fills the `epk` slot of blobs that are not ECDH-sealed (invites, channels),
/// so they keep the shape every relay and client already validates.
pub(crate) fn placeholder_epk(entropy: &mut dyn Entropy) -> Result<PublicJwk, CryptoError> {
    Ok(jwk_of(&random_secret_key(entropy)?.public_key()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use crate::{identity::Identity, rng::SeededEntropy};

    #[test]
    fn a_blob_is_remembered_by_what_its_signature_covers_not_by_the_signature_text() {
        let mut rng = SeededEntropy::new("replay");
        let (a, b) = (
            Identity::generate(&mut rng).unwrap(),
            Identity::generate(&mut rng).unwrap(),
        );
        let blob = seal(
            &a,
            &b.uid().unwrap(),
            &b.dh_public(),
            &json!({ "t": "ready" }),
            &mut rng,
        )
        .unwrap();
        let mut resigned = blob.clone();
        resigned.sig.push('A'); // another encoding of "the same" signature
        assert_eq!(replay_id(&blob), replay_id(&resigned));
        let mut other = blob.clone();
        other.ct.push('A');
        assert_ne!(replay_id(&blob), replay_id(&other));
    }

    #[test]
    fn a_friend_accepts_a_listed_device_and_nothing_else() {
        let mut rng = SeededEntropy::new("devices-seal");
        let id = |rng: &mut SeededEntropy| Identity::generate(rng).unwrap();
        let (account, phone, laptop, bob, eve) = (
            id(&mut rng),
            id(&mut rng),
            id(&mut rng),
            id(&mut rng),
            id(&mut rng),
        );
        let (au, bu) = (account.uid().unwrap(), bob.uid().unwrap());
        let one = DeviceList::first(&account, &phone, "Phone", 1).unwrap();
        let two = one
            .with_added(
                &account,
                crate::devices::DeviceEntry::new(&laptop, "Laptop", 2).unwrap(),
            )
            .unwrap();
        let laptop_id =
            crate::devices::device_id(&laptop.sign_public(), &laptop.dh_public()).unwrap();
        let fresh =
            || AccountKeys::new(au.clone(), account.sign_public(), account.dh_public(), None);
        let payload = json!({ "t": "msg", "text": "from the laptop" });
        let from_laptop = |list: &DeviceList, rng: &mut SeededEntropy| {
            seal_as(
                &Author::device(&au, &laptop, list),
                &bu,
                &bob.dh_public(),
                &payload,
                rng,
            )
            .unwrap()
        };

        // A friend who never saw the list checks the one the blob carries, and learns it.
        let mut keys = fresh();
        let opened = open(&from_laptop(&two, &mut rng), &bu, &bob, &keys).unwrap();
        assert_eq!(opened.signer, Signer::Device(laptop_id.clone()));
        assert_eq!(opened.payload, payload);
        assert!(keys.learn(opened.list.as_ref().unwrap()));
        assert!(!keys.learn(&one), "an older list changes nothing");

        // The account key still counts on its own.
        let plain = seal(&account, &bu, &bob.dh_public(), &payload, &mut rng).unwrap();
        assert_eq!(
            open(&plain, &bu, &bob, &keys).unwrap().signer,
            Signer::Account
        );

        // A device the list does not have: refused, whatever list it carries.
        let stranger = seal_as(
            &Author::device(&au, &eve, &two),
            &bu,
            &bob.dh_public(),
            &payload,
            &mut rng,
        )
        .unwrap();
        assert!(open(&stranger, &bu, &bob, &fresh()).is_none());
        // A list for another account does not vouch for anyone here.
        let eves = DeviceList::first(&eve, &laptop, "Laptop", 1).unwrap();
        assert!(open(&from_laptop(&eves, &mut rng), &bu, &bob, &fresh()).is_none());
        // A list changed in transit does not verify.
        let mut forged = two.clone();
        forged.devices[1].name = "Not a laptop".into();
        assert!(open(&from_laptop(&forged, &mut rng), &bu, &bob, &fresh()).is_none());

        // Once the friend knows the laptop was removed, an old list it still carries does not bring it back.
        let three = two.with_removed(&account, &laptop_id, 3).unwrap();
        assert!(keys.learn(&three));
        assert!(open(&from_laptop(&two, &mut rng), &bu, &bob, &keys).is_none());
        // And the blob is still only for Bob.
        assert!(open(&from_laptop(&two, &mut rng), &au, &account, &fresh()).is_none());
    }
}
