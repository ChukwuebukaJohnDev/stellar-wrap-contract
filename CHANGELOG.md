# Changelog

## Unreleased

### Added

- **Batch Mint Proxy contract** (`batch_mint_proxy`, new workspace member under `proxy/`): a separately deployable Soroban contract that forwards multiple `mint_wrap` calls to the wrap registry **atomically**. If any item in a batch fails, the entire batch is rolled back. See [`docs/batch-mint-proxy.md`](docs/batch-mint-proxy.md) for architecture, deployment, and security notes.

## 0.1.0

### Contract interface snapshot

This release documents the current public Soroban interface for the Stellar Wrap contract and records the client-facing contract changes that should be considered by backend and frontend integrations.

#### Current write methods

- `initialize(e, admin, admin_pubkey)`
- `update_admin(e, new_admin)`
- `pause(e)` / `unpause(e)`
- `migrate(e, version)`
- `mint_wrap(e, user, period, archetype, data_hash, payload_version, signature)`

#### Current read methods

- `get_wrap(e, user, period)`
- `get_mint_timestamp(e, user, period)`
- `balance_of(e, user)`
- `total_wrap_count(e)`
- `verify_data(e, user, period, data)`
- `get_latest_wrap(e, user)`
- `get_wraps(e, user, start, limit)`
- `get_admin(e)`
- `health(e)`
- `name(e)`, `symbol(e)`, `decimals(e)`
- `migration_version(e)`

### Breaking changes and migration notes

- Mint signatures are now versioned. Clients must sign the canonical payload with the current payload-versioning scheme and pass the `payload_version` argument to `mint_wrap`. Legacy integrations that only prepared signatures for the older layout should update their signer flow before deployment.
- The documented public interface now includes additional query helpers such as `get_mint_timestamp`, `total_wrap_count`, and `get_wraps`. Consumers that rely on older assumptions about the contract surface should review the current method list before building or updating clients.
- Wraps remain non-transferable and revoke semantics are not implemented. Frontends and indexers should use contract queries such as `get_wrap`, `balance_of`, and `verify_data` rather than inferring state from events alone.

### Notes for future releases

For every future entry, include a `Migration notes` subsection describing any breaking change and the required updates for client code, backend signers, and indexers.


