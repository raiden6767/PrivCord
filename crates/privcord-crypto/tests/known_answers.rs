//! Official published test vectors for the primitives our wrappers sit on, so "it works" does not rest only on the code
//! agreeing with itself and with the TypeScript and Kotlin apps. (ML-KEM-768 has no vector here yet: see
//! docs/CRYPTO_BASELINE.md.)
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice
)]

use privcord_crypto::{aead, kdf};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// RFC 5869, appendix A.1 (HKDF-SHA-256, basic test case).
#[test]
fn hkdf_sha256_matches_rfc_5869_test_case_1() {
    let ikm = hex("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
    let salt = hex("000102030405060708090a0b0c");
    let info = hex("f0f1f2f3f4f5f6f7f8f9");
    let okm: [u8; 42] = kdf::hkdf_sha256(&ikm, &salt, &info).unwrap();
    assert_eq!(
        okm.to_vec(),
        hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
    );
    // The "expand only" form takes the PRK from the same appendix.
    let prk: [u8; 32] = hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")
        .try_into()
        .unwrap();
    let again: [u8; 42] = kdf::hkdf_expand(&prk, &info).unwrap();
    assert_eq!(again.to_vec(), okm.to_vec());
}

/// McGrew and Viega, "The Galois/Counter Mode of Operation", test case 16: AES-256-GCM, 96-bit IV, with associated data.
/// Our layout is ciphertext followed by the 16-byte tag.
#[test]
fn aes_256_gcm_matches_the_published_test_case_16() {
    let key = hex("feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308");
    let iv = hex("cafebabefacedbaddecaf888");
    let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
    let plain = hex(
        "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
    );
    let cipher = hex(
        "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662",
    );
    let tag = hex("76fc6ece0f4e1768cddf8853bb2d551b");
    let sealed = aead::encrypt(&key, &iv, &plain, &aad).unwrap();
    assert_eq!(sealed, [cipher.clone(), tag.clone()].concat());
    assert_eq!(aead::decrypt(&key, &iv, &sealed, &aad).unwrap(), plain);
    // Any change is refused: a flipped bit, other associated data, a cut tag.
    let mut flipped = sealed.clone();
    flipped[0] ^= 1;
    assert!(aead::decrypt(&key, &iv, &flipped, &aad).is_err());
    assert!(aead::decrypt(&key, &iv, &sealed, b"other").is_err());
    assert!(aead::decrypt(&key, &iv, &sealed[..sealed.len() - 1], &aad).is_err());
}

/// NIST ACVP vectors for ML-KEM-768 (FIPS 203): key generation from the seed `d || z`, and encapsulation with a given
/// message `m`. A fixed entropy stream hands `m` to our wrapper, which must give exactly the published ciphertext and
/// shared secret. Then a key pair rebuilt from the seed must open what was encapsulated to its public key.
#[test]
fn ml_kem_768_matches_the_nist_acvp_vectors() {
    use privcord_crypto::{
        error::CryptoError,
        pq::{kem_encapsulate, KemKeyPair},
        rng::Entropy,
    };
    struct Fixed(Vec<u8>);
    impl Entropy for Fixed {
        fn fill(&mut self, dest: &mut [u8]) -> Result<(), CryptoError> {
            dest.copy_from_slice(&self.0[..dest.len()]);
            Ok(())
        }
    }
    let data: serde_json::Value =
        serde_json::from_str(include_str!("data/mlkem768-acvp.json")).unwrap();
    let field = |v: &serde_json::Value, name: &str| hex(v[name].as_str().unwrap());

    let keygen = data["keygen"].as_array().unwrap();
    assert!(keygen.len() >= 8);
    for t in keygen {
        let seed: [u8; 64] = [field(t, "d"), field(t, "z")].concat().try_into().unwrap();
        let pair = KemKeyPair::from_seed(&seed);
        assert_eq!(
            pair.public(),
            field(t, "ek").as_slice(),
            "key generation differs from NIST"
        );
        // Encapsulate to it and open it again with the rebuilt key pair.
        let (ct, ss) = kem_encapsulate(pair.public(), &mut Fixed(vec![7u8; 32])).unwrap();
        assert_eq!(pair.decapsulate(&ct).unwrap().to_vec(), ss.to_vec());
    }

    let encap = data["encap"].as_array().unwrap();
    assert!(encap.len() >= 8);
    for t in encap {
        let (ct, ss) = kem_encapsulate(&field(t, "ek"), &mut Fixed(field(t, "m"))).unwrap();
        assert_eq!(ct, field(t, "c"), "ciphertext differs from NIST");
        assert_eq!(
            ss.to_vec(),
            field(t, "k"),
            "shared secret differs from NIST"
        );
    }
}
