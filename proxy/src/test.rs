#![cfg(test)]

extern crate std;

use std::panic::{catch_unwind, AssertUnwindSafe};

use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::testutils::{Address as _, AuthorizedFunction, MockAuth, MockAuthInvoke};
use soroban_sdk::{symbol_short, Address, BytesN, Env, IntoVal, Symbol, Vec};

use stellar_wrap_contract::signature::construct_mint_payload;
use stellar_wrap_contract::{StellarWrapContract, StellarWrapContractClient};

use crate::{BatchMintProxy, BatchMintProxyClient, MintRequest, MAX_BATCH_SIZE};

/// Signs the canonical mint payload bound to `wrap_contract`'s address, exactly
/// as the registry's `mint_wrap` will reconstruct it during verification.
fn sign_payload(
    env: &Env,
    signer: &SigningKey,
    wrap_contract: &Address,
    user: &Address,
    period: u64,
    archetype: &Symbol,
    data_hash: &BytesN<32>,
) -> BytesN<64> {
    let payload = construct_mint_payload(env, wrap_contract, user, period, archetype, data_hash, 1);

    let mut out = [0u8; 512];
    let len = payload.len() as usize;
    payload.copy_into_slice(&mut out[..len]);

    let signature = signer.sign(&out[..len]);
    BytesN::from_array(env, &signature.to_bytes())
}

struct Fixture {
    env: Env,
    wrap_id: Address,
    proxy_id: Address,
    signing_key: SigningKey,
}

impl Fixture {
    fn wrap(&self) -> StellarWrapContractClient<'_> {
        StellarWrapContractClient::new(&self.env, &self.wrap_id)
    }

    fn proxy(&self) -> BatchMintProxyClient<'_> {
        BatchMintProxyClient::new(&self.env, &self.proxy_id)
    }
}

/// Deploys a real wrap registry plus a proxy pointing at it, both configured
/// with the same admin key. Auths are mocked to allow the registry's
/// `user.require_auth()` calls to happen in sub-invocations of the proxy.
fn setup() -> Fixture {
    let env = Env::default();
    let wrap_id = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());
    let wrap = StellarWrapContractClient::new(&env, &wrap_id);
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);

    let signing_key = SigningKey::from_bytes(&[42u8; 32]);
    let admin_pubkey = BytesN::from_array(&env, &signing_key.verifying_key().to_bytes());
    let admin = Address::generate(&env);

    wrap.initialize(&admin, &admin_pubkey);
    proxy.initialize(&admin, &wrap_id);

    // The registry's `mint_wrap` calls `user.require_auth()` inside a
    // sub-invocation of the proxy, so plain `mock_all_auths()` would reject it.
    env.mock_all_auths_allowing_non_root_auth();

    Fixture {
        env,
        wrap_id,
        proxy_id,
        signing_key,
    }
}

/// Builds a valid, signed mint request for `user`/`period`.
fn valid_item(
    f: &Fixture,
    user: &Address,
    period: u64,
    archetype: &Symbol,
    data_hash: &BytesN<32>,
) -> MintRequest {
    MintRequest {
        user: user.clone(),
        period,
        archetype: archetype.clone(),
        data_hash: data_hash.clone(),
        payload_version: 1,
        signature: sign_payload(
            &f.env,
            &f.signing_key,
            &f.wrap_id,
            user,
            period,
            archetype,
            data_hash,
        ),
    }
}

/// Builds a `MintRequest` whose signature is deliberately invalid. The period
/// and user are still well-formed so the registry's only failure is the
/// signature check.
fn invalid_signature_item(f: &Fixture, user: &Address, period: u64) -> MintRequest {
    MintRequest {
        user: user.clone(),
        period,
        archetype: symbol_short!("arch"),
        data_hash: BytesN::from_array(&f.env, &[9u8; 32]),
        payload_version: 1,
        signature: BytesN::from_array(&f.env, &[0u8; 64]),
    }
}

fn to_vec(env: &Env, items: std::vec::Vec<MintRequest>) -> Vec<MintRequest> {
    let mut v = Vec::new(env);
    for item in items {
        v.push_back(item);
    }
    v
}

#[test]
fn test_initialize_sets_admin_and_wrap_contract() {
    let env = Env::default();
    let wrap_id = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);
    let admin = Address::generate(&env);

    proxy.initialize(&admin, &wrap_id);

    assert_eq!(proxy.admin(), Some(admin.clone()));
    assert_eq!(proxy.wrap_contract(), Some(wrap_id));
}

#[test]
fn test_double_initialize_panics() {
    let env = Env::default();
    let wrap_id = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);
    let admin = Address::generate(&env);

    proxy.initialize(&admin, &wrap_id);

    let result = catch_unwind(AssertUnwindSafe(|| proxy.initialize(&admin, &wrap_id)));
    assert!(result.is_err(), "double initialization must panic");
}

#[test]
fn test_batch_mint_wrap_before_initialize_panics() {
    let env = Env::default();
    let proxy_id = env.register(BatchMintProxy, ());
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);
    env.mock_all_auths_allowing_non_root_auth();

    let user = Address::generate(&env);
    let item = MintRequest {
        user: user.clone(),
        period: 202601,
        archetype: symbol_short!("arch"),
        data_hash: BytesN::from_array(&env, &[1u8; 32]),
        payload_version: 1,
        signature: BytesN::from_array(&env, &[0u8; 64]),
    };

    let items = to_vec(&env, std::vec![item]);
    let result = catch_unwind(AssertUnwindSafe(|| proxy.batch_mint_wrap(&items)));
    assert!(
        result.is_err(),
        "batch_mint_wrap on an uninitialized proxy must panic"
    );
}

#[test]
fn test_batch_mint_wrap_mints_every_item() {
    let f = setup();

    let user1 = Address::generate(&f.env);
    let user2 = Address::generate(&f.env);
    let user3 = Address::generate(&f.env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&f.env, &[7u8; 32]);

    let item1 = valid_item(&f, &user1, 202601, &archetype, &hash);
    let item2 = valid_item(&f, &user2, 202602, &archetype, &hash);
    let item3 = valid_item(&f, &user3, 202603, &archetype, &hash);

    let count = f
        .proxy()
        .batch_mint_wrap(&to_vec(&f.env, std::vec![item1, item2, item3]));

    assert_eq!(count, 3);
    assert!(f.wrap().get_wrap(&user1, &202601).is_some());
    assert!(f.wrap().get_wrap(&user2, &202602).is_some());
    assert!(f.wrap().get_wrap(&user3, &202603).is_some());
    assert_eq!(f.wrap().total_wrap_count(), 3);

    let wrap = f.wrap().get_wrap(&user1, &202601).unwrap();
    assert_eq!(wrap.archetype, archetype);
    assert_eq!(wrap.data_hash, hash);
}

#[test]
fn test_single_item_batch_mints_one_wrap() {
    let f = setup();
    let user = Address::generate(&f.env);
    let archetype = symbol_short!("gold");
    let hash = BytesN::from_array(&f.env, &[3u8; 32]);
    let item = valid_item(&f, &user, 202605, &archetype, &hash);

    let count = f.proxy().batch_mint_wrap(&to_vec(&f.env, std::vec![item]));

    assert_eq!(count, 1);
    assert!(f.wrap().get_wrap(&user, &202605).is_some());
}

#[test]
fn test_batch_mint_wrap_records_per_user_registry_authorization() {
    let f = setup();
    let user1 = Address::generate(&f.env);
    let user2 = Address::generate(&f.env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&f.env, &[5u8; 32]);
    let item1 = valid_item(&f, &user1, 202601, &archetype, &hash);
    let item2 = valid_item(&f, &user2, 202602, &archetype, &hash);

    f.proxy()
        .batch_mint_wrap(&to_vec(&f.env, std::vec![item1, item2]));

    let auths = f.env.auths();
    assert_eq!(
        auths.len(),
        2,
        "each user must authorize the registry's mint_wrap sub-invocation"
    );

    let mut authed_users: std::vec::Vec<&Address> = auths.iter().map(|(a, _)| a).collect();
    authed_users.sort_by_key(|a| a.to_string());
    let mut expected: std::vec::Vec<&Address> = std::vec![&user1, &user2];
    expected.sort_by_key(|a| a.to_string());
    assert_eq!(authed_users, expected);

    for (_, invocation) in auths {
        match invocation.function {
            AuthorizedFunction::Contract((contract, fn_name, _args)) => {
                assert_eq!(contract, f.wrap_id.clone());
                assert_eq!(fn_name, Symbol::new(&f.env, "mint_wrap"));
            }
            other => panic!("unexpected authorized function: {other:?}"),
        }
        assert!(invocation.sub_invocations.is_empty());
    }
}

#[test]
fn test_batch_is_atomic_when_an_item_has_an_invalid_signature() {
    let f = setup();
    let user1 = Address::generate(&f.env);
    let user2 = Address::generate(&f.env);
    let user3 = Address::generate(&f.env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&f.env, &[2u8; 32]);

    let ok1 = valid_item(&f, &user1, 202601, &archetype, &hash);
    let bad = invalid_signature_item(&f, &user2, 202602);
    let ok3 = valid_item(&f, &user3, 202603, &archetype, &hash);

    let result = catch_unwind(AssertUnwindSafe(|| {
        f.proxy()
            .batch_mint_wrap(&to_vec(&f.env, std::vec![ok1, bad, ok3]))
    }));

    assert!(result.is_err(), "a bad signature must fail the whole batch");

    // Nothing may have been committed, including the valid items that were
    // forwarded before the failing one.
    assert!(f.wrap().get_wrap(&user1, &202601).is_none());
    assert!(f.wrap().get_wrap(&user2, &202602).is_none());
    assert!(f.wrap().get_wrap(&user3, &202603).is_none());
    assert_eq!(f.wrap().total_wrap_count(), 0);
}

#[test]
fn test_batch_is_atomic_on_duplicate_user_period() {
    let f = setup();
    let user1 = Address::generate(&f.env);
    let user2 = Address::generate(&f.env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&f.env, &[4u8; 32]);

    // user1/202601 appears twice; the second occurrence must fail with
    // WrapAlreadyExists and roll the first (and everything after) back.
    let dup_a = valid_item(&f, &user1, 202601, &archetype, &hash);
    let dup_b = valid_item(&f, &user1, 202601, &archetype, &hash);
    let ok = valid_item(&f, &user2, 202602, &archetype, &hash);

    let result = catch_unwind(AssertUnwindSafe(|| {
        f.proxy()
            .batch_mint_wrap(&to_vec(&f.env, std::vec![dup_a, dup_b, ok]))
    }));

    assert!(
        result.is_err(),
        "duplicate (user, period) must fail the batch"
    );
    assert!(f.wrap().get_wrap(&user1, &202601).is_none());
    assert!(f.wrap().get_wrap(&user2, &202602).is_none());
    assert_eq!(f.wrap().total_wrap_count(), 0);
}

#[test]
fn test_batch_is_atomic_on_opted_out_user() {
    let f = setup();
    let user1 = Address::generate(&f.env);
    let user2 = Address::generate(&f.env);
    let user3 = Address::generate(&f.env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&f.env, &[6u8; 32]);

    f.wrap().opt_out(&user2);

    let ok1 = valid_item(&f, &user1, 202601, &archetype, &hash);
    let opted_out = valid_item(&f, &user2, 202602, &archetype, &hash);
    let ok3 = valid_item(&f, &user3, 202603, &archetype, &hash);

    let result = catch_unwind(AssertUnwindSafe(|| {
        f.proxy()
            .batch_mint_wrap(&to_vec(&f.env, std::vec![ok1, opted_out, ok3]))
    }));

    assert!(
        result.is_err(),
        "an opted-out user must fail the whole batch"
    );
    assert!(f.wrap().get_wrap(&user1, &202601).is_none());
    assert!(f.wrap().get_wrap(&user2, &202602).is_none());
    assert!(f.wrap().get_wrap(&user3, &202603).is_none());
    assert_eq!(f.wrap().total_wrap_count(), 0);
}

#[test]
fn test_batch_rolls_back_when_registry_is_paused() {
    let f = setup();
    let user = Address::generate(&f.env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&f.env, &[8u8; 32]);

    f.wrap().pause();

    let item = valid_item(&f, &user, 202601, &archetype, &hash);
    let result = catch_unwind(AssertUnwindSafe(|| {
        f.proxy().batch_mint_wrap(&to_vec(&f.env, std::vec![item]))
    }));

    assert!(result.is_err(), "a paused registry must fail the batch");
    assert!(f.wrap().get_wrap(&user, &202601).is_none());
    assert_eq!(f.wrap().total_wrap_count(), 0);
}

#[test]
fn test_empty_batch_is_rejected() {
    let f = setup();
    let items = Vec::new(&f.env);

    let result = catch_unwind(AssertUnwindSafe(|| f.proxy().batch_mint_wrap(&items)));
    assert!(result.is_err(), "an empty batch must be rejected");
}

#[test]
fn test_oversized_batch_is_rejected() {
    let f = setup();

    let mut raw = std::vec::Vec::new();
    for i in 0..(MAX_BATCH_SIZE + 1) {
        // Contents don't matter: the size check runs before any forwarding, so
        // dummy signatures are acceptable here.
        raw.push(MintRequest {
            user: Address::generate(&f.env),
            period: 202601 + (i % 12) as u64,
            archetype: symbol_short!("arch"),
            data_hash: BytesN::from_array(&f.env, &[0u8; 32]),
            payload_version: 1,
            signature: BytesN::from_array(&f.env, &[0u8; 64]),
        });
    }
    let items = to_vec(&f.env, raw);

    let result = catch_unwind(AssertUnwindSafe(|| f.proxy().batch_mint_wrap(&items)));
    assert!(
        result.is_err(),
        "a batch larger than MAX_BATCH_SIZE must be rejected"
    );
    assert_eq!(f.wrap().total_wrap_count(), 0);
}

#[test]
fn test_set_wrap_contract_requires_authorization() {
    let env = Env::default();
    let wrap_id = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);
    let admin = Address::generate(&env);
    let new_target = Address::generate(&env);

    proxy.initialize(&admin, &wrap_id);

    // No auth mocked at all → must fail.
    let result = catch_unwind(AssertUnwindSafe(|| proxy.set_wrap_contract(&new_target)));
    assert!(result.is_err(), "set_wrap_contract without auth must fail");
}

#[test]
fn test_set_wrap_contract_rejects_non_admin() {
    let env = Env::default();
    let wrap_id = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);
    let admin = Address::generate(&env);
    let attacker = Address::generate(&env);
    let new_target = Address::generate(&env);

    proxy.initialize(&admin, &wrap_id);

    env.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &proxy_id,
            fn_name: "set_wrap_contract",
            args: (&new_target,).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    let result = catch_unwind(AssertUnwindSafe(|| proxy.set_wrap_contract(&new_target)));
    assert!(
        result.is_err(),
        "a non-admin must not be able to retarget the proxy"
    );
    assert_eq!(proxy.wrap_contract(), Some(wrap_id));
}

#[test]
fn test_set_wrap_contract_succeeds_for_admin() {
    let env = Env::default();
    let wrap_id = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);
    let admin = Address::generate(&env);
    let new_target = Address::generate(&env);

    proxy.initialize(&admin, &wrap_id);

    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &proxy_id,
            fn_name: "set_wrap_contract",
            args: (&new_target,).into_val(&env),
            sub_invokes: &[],
        },
    }]);

    proxy.set_wrap_contract(&new_target);
    assert_eq!(proxy.wrap_contract(), Some(new_target.clone()));
}

#[test]
fn test_set_wrap_contract_redirects_batches_to_new_registry() {
    let env = Env::default();
    let signing_key = SigningKey::from_bytes(&[77u8; 32]);
    let admin_pubkey = BytesN::from_array(&env, &signing_key.verifying_key().to_bytes());
    let admin = Address::generate(&env);

    let wrap1 = env.register(StellarWrapContract, ());
    let wrap2 = env.register(StellarWrapContract, ());
    let proxy_id = env.register(BatchMintProxy, ());

    let wrap1_client = StellarWrapContractClient::new(&env, &wrap1);
    let wrap2_client = StellarWrapContractClient::new(&env, &wrap2);
    let proxy = BatchMintProxyClient::new(&env, &proxy_id);

    wrap1_client.initialize(&admin, &admin_pubkey);
    wrap2_client.initialize(&admin, &admin_pubkey);
    proxy.initialize(&admin, &wrap1);

    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &proxy_id,
            fn_name: "set_wrap_contract",
            args: (&wrap2,).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    proxy.set_wrap_contract(&wrap2);

    env.mock_all_auths_allowing_non_root_auth();

    let user = Address::generate(&env);
    let archetype = symbol_short!("arch");
    let hash = BytesN::from_array(&env, &[1u8; 32]);
    let payload = construct_mint_payload(&env, &wrap2, &user, 202601, &archetype, &hash, 1);
    let mut out = [0u8; 512];
    let len = payload.len() as usize;
    payload.copy_into_slice(&mut out[..len]);
    let sig = signing_key.sign(&out[..len]);
    let signature = BytesN::from_array(&env, &sig.to_bytes());

    let item = MintRequest {
        user: user.clone(),
        period: 202601,
        archetype: archetype.clone(),
        data_hash: hash.clone(),
        payload_version: 1,
        signature,
    };

    let count = proxy.batch_mint_wrap(&to_vec(&env, std::vec![item]));
    assert_eq!(count, 1);

    // The wrap must land on the new registry, not the original one.
    assert!(wrap2_client.get_wrap(&user, &202601).is_some());
    assert!(wrap1_client.get_wrap(&user, &202601).is_none());
    assert_eq!(wrap1_client.total_wrap_count(), 0);
    assert_eq!(wrap2_client.total_wrap_count(), 1);
}
