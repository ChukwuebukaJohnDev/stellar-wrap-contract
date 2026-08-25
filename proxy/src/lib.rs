//! # Batch Mint Proxy
//!
//! A lightweight Soroban contract that batches multiple `mint_wrap` calls to
//! the Stellar Wrap Registry (`stellar_wrap_contract`) into a single, atomic
//! invocation.
//!
//! ## Why a proxy?
//!
//! The wrap registry exposes `mint_wrap` for individual wraps and
//! `mint_wrap_batch` for in-contract batching. The batch-mint proxy provides a
//! third deployment option: a **separately deployable contract** that forwards
//! ordinary `mint_wrap` calls one-by-one. Because a Soroban invocation is
//! transactional, if any forwarded call fails (bad signature, duplicate wrap,
//! paused registry, etc.) the whole transaction — including every earlier
//! wrap in the batch — is rolled back. This gives callers atomic
//! all-or-nothing semantics without changing the registry contract.
//!
//! ## Authorization
//!
//! The proxy itself performs no `require_auth` for end users. Every security
//! check stays in the wrap registry: each forwarded `mint_wrap` call runs the
//! registry's own Ed25519 signature verification and its `user.require_auth()`.
//! Consequently a batch transaction must carry a Soroban authorization entry
//! per user authorizing the registry's `mint_wrap` sub-invocation (with the
//! proxy as the calling contract). The admin of the proxy may only change the
//! wrapped registry address via [`BatchMintProxy::set_wrap_contract`].
//!
//! ## Atomicity
//!
//! If any item in the batch fails to mint, the entire batch rolls back. The
//! proxy does not catch or swallow sub-call errors; it relies on the Soroban
//! host's transactional rollback. See [`BatchMintProxy::batch_mint_wrap`].

#![no_std]

#[cfg(any(test, feature = "testutils"))]
extern crate std;

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error, Address,
    BytesN, Env, Symbol, Vec,
};

/// Maximum number of `mint_wrap` calls forwarded in a single batch.
///
/// Mirrors the registry's own `mint::MAX_BATCH_SIZE` (100). Kept separate so
/// the proxy can be tuned independently without a registry upgrade.
pub const MAX_BATCH_SIZE: u32 = 100;

const TTL_ONE_YEAR: u32 = 17_280 * 365;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ProxyError {
    /// `initialize` has not been called yet.
    NotInitialized = 1,
    /// `initialize` has already been called.
    AlreadyInitialized = 2,
    /// The caller is not the configured proxy admin.
    Unauthorized = 3,
    /// The batch contains zero items.
    EmptyBatch = 4,
    /// The batch exceeds `MAX_BATCH_SIZE` items.
    BatchTooLarge = 5,
}

/// A single `mint_wrap` request to forward to the wrap registry.
///
/// Fields mirror the registry's `mint_wrap` signature. The `signature` is the
/// admin Ed25519 signature over the canonical payload bound to the **wrap
/// registry's** contract address (not the proxy's).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MintRequest {
    pub user: Address,
    pub period: u64,
    pub archetype: Symbol,
    pub data_hash: BytesN<32>,
    pub payload_version: u32,
    pub signature: BytesN<64>,
}

/// Minimal ABI of the wrap registry's `mint_wrap` entrypoint.
#[contractclient(name = "WrapMintClient")]
pub trait WrapMint {
    fn mint_wrap(
        e: Env,
        user: Address,
        period: u64,
        archetype: Symbol,
        data_hash: BytesN<32>,
        payload_version: u32,
        signature: BytesN<64>,
    );
}

#[contracttype]
enum DataKey {
    /// Address allowed to change the wrapped registry.
    Admin,
    /// Address of the wrap registry this proxy forwards to.
    WrapContract,
}

#[contract]
pub struct BatchMintProxy;

#[contractimpl]
impl BatchMintProxy {
    /// Configure the proxy with an `admin` (who may later point the proxy at a
    /// different registry) and the `wrap_contract` address that receives all
    /// forwarded `mint_wrap` calls.
    ///
    /// Can only be called once; a second call panics with
    /// [`ProxyError::AlreadyInitialized`].
    pub fn initialize(e: Env, admin: Address, wrap_contract: Address) {
        if e.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(e, ProxyError::AlreadyInitialized);
        }
        e.storage().instance().set(&DataKey::Admin, &admin);
        e.storage()
            .instance()
            .set(&DataKey::WrapContract, &wrap_contract);
        e.storage()
            .instance()
            .extend_ttl(TTL_ONE_YEAR, TTL_ONE_YEAR);
    }

    /// Re-point the proxy at a different wrap registry.
    ///
    /// Only the configured admin may call this. Useful when the registry is
    /// redeployed or a new canonical registry address is chosen.
    ///
    /// # Panics
    /// - [`ProxyError::NotInitialized`] if the proxy has not been initialized.
    /// - [`ProxyError::Unauthorized`] if the caller is not the admin.
    pub fn set_wrap_contract(e: Env, wrap_contract: Address) {
        let admin: Address = e
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(e, ProxyError::NotInitialized));
        admin.require_auth();

        e.storage()
            .instance()
            .set(&DataKey::WrapContract, &wrap_contract);
        e.storage()
            .instance()
            .extend_ttl(TTL_ONE_YEAR, TTL_ONE_YEAR);
    }

    /// Batch multiple `mint_wrap` calls to the wrapped registry atomically.
    ///
    /// Each item is forwarded to the registry's `mint_wrap` in order. The
    /// registry enforces per-user authorization, Ed25519 signature validity,
    /// period/payload validation, and duplicate checks. If **any** item fails,
    /// the Soroban host rolls back the entire transaction, so no partial batch
    /// can ever be committed.
    ///
    /// # Arguments
    /// - `items` — the ordered list of `mint_wrap` requests (1..=`MAX_BATCH_SIZE`).
    ///
    /// # Returns
    /// The number of minted wraps, which equals `items.len()` on success.
    ///
    /// # Panics
    /// - [`ProxyError::NotInitialized`] if the proxy has not been initialized.
    /// - [`ProxyError::EmptyBatch`] if `items` is empty.
    /// - [`ProxyError::BatchTooLarge`] if `items.len() > MAX_BATCH_SIZE`.
    /// - Any error propagated from the wrapped registry's `mint_wrap`.
    pub fn batch_mint_wrap(e: Env, items: Vec<MintRequest>) -> u32 {
        let wrap_contract: Address = e
            .storage()
            .instance()
            .get(&DataKey::WrapContract)
            .unwrap_or_else(|| panic_with_error!(e, ProxyError::NotInitialized));

        if items.is_empty() {
            panic_with_error!(e, ProxyError::EmptyBatch);
        }
        if items.len() > MAX_BATCH_SIZE {
            panic_with_error!(e, ProxyError::BatchTooLarge);
        }

        // Forward every request to the registry. Any failing sub-call panics
        // and rolls back the entire invocation (atomic all-or-nothing).
        for item in items.iter() {
            WrapMintClient::new(&e, &wrap_contract).mint_wrap(
                &item.user,
                &item.period,
                &item.archetype,
                &item.data_hash,
                &item.payload_version,
                &item.signature,
            );
        }

        // Keep the proxy's instance config alive while the proxy is in use.
        e.storage()
            .instance()
            .extend_ttl(TTL_ONE_YEAR, TTL_ONE_YEAR);

        items.len()
    }

    /// Return the configured proxy admin, or `None` before initialization.
    pub fn admin(e: Env) -> Option<Address> {
        e.storage().instance().get(&DataKey::Admin)
    }

    /// Return the wrap registry address this proxy forwards to, or `None`
    /// before initialization.
    pub fn wrap_contract(e: Env) -> Option<Address> {
        e.storage().instance().get(&DataKey::WrapContract)
    }
}

#[cfg(test)]
mod test;
