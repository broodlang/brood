//! Cryptographic primitives: digests, HMAC, authenticated encryption, key
//! derivation, and a CSPRNG. Rust *mechanism* only — the single place the kernel
//! reaches for the `md5`/`sha1`/`sha2`/`hmac`/`chacha20poly1305` crates; all
//! string-input / hex-output / nonce-generation *policy* is Brood in
//! `std/hash.blsp` / `std/crypto.blsp`. Raw bytes in, raw `bytes` out (via
//! [`bytes_to_value`]), so digests chain without a hex round-trip at each step.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::io::{bytes_to_value, collect_bytes};
use super::numeric::{arg, expect_int};

/// Hash algorithm selector for `%digest` / `%hmac`, decoded from the leading
/// keyword arg. This is the single place the kernel enumerates digest
/// algorithms; all string-input and hex-output shaping is Brood policy in
/// `std/hash.blsp` (over `string->utf8-bytes` and `bytes->hex`).
#[derive(Clone, Copy)]
enum HashAlgo {
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

fn hash_algo(name: &'static str, kw: Value, heap: &mut Heap) -> Result<HashAlgo, LispError> {
    let sym = match kw {
        Value::Keyword(s) => s,
        other => {
            return Err(LispError::wrong_type(
                heap,
                name,
                "algorithm keyword",
                other,
            ))
        }
    };
    match value::symbol_name(sym).as_str() {
        "md5" => Ok(HashAlgo::Md5),
        "sha1" => Ok(HashAlgo::Sha1),
        "sha256" => Ok(HashAlgo::Sha256),
        "sha384" => Ok(HashAlgo::Sha384),
        "sha512" => Ok(HashAlgo::Sha512),
        other => Err(LispError::runtime(format!(
            "{name}: unknown algorithm :{other} (want :md5 :sha1 :sha256 :sha384 :sha512)"
        ))),
    }
}

/// `(%digest algo bytes)` — raw digest of a byte sequence (`bytes` value, vector,
/// or list of byte ints) under algorithm keyword `algo`, returned as a bytes
/// value. The single digest primitive: string-input and hex-output variants are
/// Brood wrappers in `std/hash.blsp` (collapsed the former 15 `%sha*`/`%md5`
/// prims to this one — ADR-006 / dogfooding).
pub(super) fn digest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let algo = hash_algo("%digest", arg(args, 0), heap)?;
    let bytes = collect_bytes("%digest", arg(args, 1), heap)?;
    let out: Vec<u8> = match algo {
        HashAlgo::Md5 => {
            use md5::{Digest, Md5};
            Md5::digest(&bytes).to_vec()
        }
        HashAlgo::Sha1 => {
            use sha1::{Digest, Sha1};
            Sha1::digest(&bytes).to_vec()
        }
        HashAlgo::Sha256 => {
            use sha2::{Digest, Sha256};
            Sha256::digest(&bytes).to_vec()
        }
        HashAlgo::Sha384 => {
            use sha2::{Digest, Sha384};
            Sha384::digest(&bytes).to_vec()
        }
        HashAlgo::Sha512 => {
            use sha2::{Digest, Sha512};
            Sha512::digest(&bytes).to_vec()
        }
    };
    Ok(bytes_to_value(&out, heap))
}

/// `(%hmac algo key-bytes msg-bytes)` — HMAC of `msg-bytes` keyed by `key-bytes`
/// (both byte sequences) under algorithm keyword `algo`, returned as a bytes
/// value. String-keyed / hex-output variants are Brood wrappers in
/// `std/hash.blsp` (collapsed the former 6 `%hmac-*` prims to this one).
pub(super) fn hmac(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    let algo = hash_algo("%hmac", arg(args, 0), heap)?;
    let key = collect_bytes("%hmac", arg(args, 1), heap)?;
    let msg = collect_bytes("%hmac", arg(args, 2), heap)?;
    let mac_err = |e| LispError::runtime(format!("%hmac: {e}"));
    let out: Vec<u8> = match algo {
        HashAlgo::Md5 => {
            use md5::Md5;
            let mut mac = Hmac::<Md5>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha1 => {
            use sha1::Sha1;
            let mut mac = Hmac::<Sha1>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha256 => {
            use sha2::Sha256;
            let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha384 => {
            use sha2::Sha384;
            let mut mac = Hmac::<Sha384>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
        HashAlgo::Sha512 => {
            use sha2::Sha512;
            let mut mac = Hmac::<Sha512>::new_from_slice(&key).map_err(mac_err)?;
            mac.update(&msg);
            mac.finalize().into_bytes().to_vec()
        }
    };
    Ok(bytes_to_value(&out, heap))
}

/// `(%random-bytes n)` — `n` cryptographically-strong random bytes as a Brood
/// bytes value. Useful for generating keys, nonces, and salts.
pub(super) fn random_bytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "%random-bytes", arg(args, 0))?;
    if !(0..=65536).contains(&n) {
        return Err(LispError::runtime(
            "%random-bytes: byte count must be in 0..=65536",
        ));
    }
    let mut bytes = vec![0u8; n as usize];
    getrandom::fill(&mut bytes)
        .map_err(|e| LispError::runtime(format!("%random-bytes: OS RNG unavailable: {e}")))?;
    Ok(bytes_to_value(&bytes, heap))
}

/// `(%ed25519-keygen)` — a fresh ed25519 keypair as a two-element vector
/// `[public private]`: `public` the 32-byte verifying key, `private` the 32-byte
/// signing seed, both `bytes` values. The single key-generation primitive for
/// package signing (ADR-212); hex encoding, on-disk storage, and the publish/verify
/// flow are Brood policy in `std/tool/package.blsp`.
pub(super) fn ed25519_keygen(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use ed25519_dalek::SigningKey;
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|e| LispError::runtime(format!("%ed25519-keygen: OS RNG unavailable: {e}")))?;
    let signing = SigningKey::from_bytes(&seed);
    let public = signing.verifying_key().to_bytes();
    let public_value = bytes_to_value(public, heap);
    let private_value = bytes_to_value(seed, heap);
    Ok(heap.alloc_vector(vec![public_value, private_value]))
}

/// `(%ed25519-sign private-bytes message-bytes)` — the 64-byte ed25519 signature of
/// `message-bytes` under the 32-byte `private-bytes` signing seed, as a `bytes`
/// value. Errors only when the key is not 32 bytes (a programming error); the
/// signature itself never fails.
pub(super) fn ed25519_sign(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use ed25519_dalek::{Signer, SigningKey};
    let seed_bytes = collect_bytes("%ed25519-sign", arg(args, 0), heap)?;
    let message = collect_bytes("%ed25519-sign", arg(args, 1), heap)?;
    let seed: [u8; 32] = seed_bytes.as_slice().try_into().map_err(|_| {
        LispError::runtime(format!(
            "%ed25519-sign: private key must be 32 bytes, got {}",
            seed_bytes.len()
        ))
    })?;
    let signing = SigningKey::from_bytes(&seed);
    let signature = signing.sign(&message);
    Ok(bytes_to_value(signature.to_bytes(), heap))
}

/// `(%ed25519-verify public-bytes message-bytes signature-bytes)` — `true` when
/// `signature-bytes` (64 bytes) is a valid ed25519 signature of `message-bytes` under
/// the 32-byte `public-bytes` key, else `false`. Verification never errors: a
/// malformed key/signature or a bad signature is simply `false`, so a caller treats
/// it as a plain predicate.
pub(super) fn ed25519_verify(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let public = collect_bytes("%ed25519-verify", arg(args, 0), heap)?;
    let message = collect_bytes("%ed25519-verify", arg(args, 1), heap)?;
    let signature = collect_bytes("%ed25519-verify", arg(args, 2), heap)?;
    let ok = (|| {
        let public: [u8; 32] = public.as_slice().try_into().ok()?;
        let signature: [u8; 64] = signature.as_slice().try_into().ok()?;
        let verifying = VerifyingKey::from_bytes(&public).ok()?;
        verifying
            .verify(&message, &Signature::from_bytes(&signature))
            .ok()
    })()
    .is_some();
    Ok(Value::Bool(ok))
}

/// `(%chacha20-encrypt key-bytes nonce-bytes plaintext-bytes)` — authenticated
/// encryption (ChaCha20-Poly1305). `key-bytes` must be exactly 32 bytes;
/// `nonce-bytes` must be exactly 12 bytes. Returns the ciphertext (plaintext
/// length + 16-byte Poly1305 authentication tag) as a byte vector.
///
/// **NEVER reuse a (key, nonce) pair.** A fresh nonce is required per message —
/// reuse breaks both confidentiality *and* the Poly1305 integrity guarantee.
/// Nonce generation is the caller's responsibility (see `crypto/random-nonce`).
pub(super) fn chacha20_encrypt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
    let key_bytes = collect_bytes("%chacha20-encrypt", arg(args, 0), heap)?;
    let nonce_bytes = collect_bytes("%chacha20-encrypt", arg(args, 1), heap)?;
    let plaintext = collect_bytes("%chacha20-encrypt", arg(args, 2), heap)?;
    if key_bytes.len() != 32 {
        return Err(LispError::runtime(format!(
            "%chacha20-encrypt: key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    if nonce_bytes.len() != 12 {
        return Err(LispError::runtime(format!(
            "%chacha20-encrypt: nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| LispError::runtime(format!("%chacha20-encrypt: {e}")))?;
    let nonce = Nonce::try_from(nonce_bytes.as_slice())
        .map_err(|e| LispError::runtime(format!("chacha20 nonce: {e}")))?;
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_slice())
        .map_err(|e| LispError::runtime(format!("%chacha20-encrypt: {e}")))?;
    Ok(bytes_to_value(&ciphertext, heap))
}

/// `(%chacha20-decrypt key-bytes nonce-bytes ciphertext-bytes)` — authenticated
/// decryption (ChaCha20-Poly1305). Returns the plaintext as a byte vector, or
/// `:error` if the authentication tag fails (tampered or wrong key/nonce).
pub(super) fn chacha20_decrypt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
    let key_bytes = collect_bytes("%chacha20-decrypt", arg(args, 0), heap)?;
    let nonce_bytes = collect_bytes("%chacha20-decrypt", arg(args, 1), heap)?;
    let ciphertext = collect_bytes("%chacha20-decrypt", arg(args, 2), heap)?;
    if key_bytes.len() != 32 {
        return Err(LispError::runtime(format!(
            "%chacha20-decrypt: key must be 32 bytes, got {}",
            key_bytes.len()
        )));
    }
    if nonce_bytes.len() != 12 {
        return Err(LispError::runtime(format!(
            "%chacha20-decrypt: nonce must be 12 bytes, got {}",
            nonce_bytes.len()
        )));
    }
    let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
        .map_err(|e| LispError::runtime(format!("%chacha20-decrypt: {e}")))?;
    let nonce = Nonce::try_from(nonce_bytes.as_slice())
        .map_err(|e| LispError::runtime(format!("chacha20 nonce: {e}")))?;
    match cipher.decrypt(&nonce, ciphertext.as_slice()) {
        Ok(plaintext) => Ok(bytes_to_value(&plaintext, heap)),
        Err(_) => Ok(Value::keyword(value::intern("error"))),
    }
}

/// `(%pbkdf2-sha256-bytes password-bytes salt-bytes iterations key-len)` — derive
/// a key from a password using PBKDF2-HMAC-SHA256 (RFC 2898). `password-bytes`
/// and `salt-bytes` are byte vectors (raw bytes, not UTF-8-decoded strings — so
/// a base64-decoded binary salt round-trips faithfully, store-driver finding #4).
/// Returns a bytes value of `key-len` bytes. Use `iterations` ≥ 600,000 for
/// password storage (NIST SP 800-132 2023). Implemented over the `hmac` + `sha2`
/// crates — microseconds where the pure-Brood version cost ~2s/connection (#5).
pub(super) fn pbkdf2_sha256_fn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let pw = collect_bytes("%pbkdf2-sha256-bytes", arg(args, 0), heap)?;
    let salt = collect_bytes("%pbkdf2-sha256-bytes", arg(args, 1), heap)?;
    let iterations = expect_int(heap, "%pbkdf2-sha256-bytes", arg(args, 2))?;
    let key_len = expect_int(heap, "%pbkdf2-sha256-bytes", arg(args, 3))?;
    if iterations <= 0 {
        return Err(LispError::runtime(
            "%pbkdf2-sha256-bytes: iterations must be positive",
        ));
    }
    if !(1..=512).contains(&key_len) {
        return Err(LispError::runtime(
            "%pbkdf2-sha256-bytes: key-len must be in 1..=512",
        ));
    }
    let hlen = 32usize; // SHA-256 output bytes
    let block_count = (key_len as usize).div_ceil(hlen);
    let mut dk = Vec::with_capacity(key_len as usize);
    for i in 1u32..=(block_count as u32) {
        // U_1 = HMAC(password, salt || INT(i))
        let mut mac = HmacSha256::new_from_slice(&pw)
            .map_err(|e| LispError::runtime(format!("%pbkdf2-sha256-bytes: {e}")))?;
        mac.update(&salt);
        mac.update(&i.to_be_bytes());
        let mut u: Vec<u8> = mac.finalize().into_bytes().to_vec();
        let mut t = u.clone();
        // U_n = HMAC(password, U_{n-1}); T_i = XOR of all U_j
        for _ in 1..(iterations as u32) {
            let mut mac2 = HmacSha256::new_from_slice(&pw)
                .map_err(|e| LispError::runtime(format!("%pbkdf2-sha256-bytes: {e}")))?;
            mac2.update(&u);
            u = mac2.finalize().into_bytes().to_vec();
            for j in 0..hlen {
                t[j] ^= u[j];
            }
        }
        dk.extend_from_slice(&t);
    }
    dk.truncate(key_len as usize);
    Ok(bytes_to_value(&dk, heap))
}
