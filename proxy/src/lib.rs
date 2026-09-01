#no_std

use soroban_sdk::{contract, contractimpl, Address, Env, IntoVal, Symbol, Vec}

pub mod errors;
pub mod events;
pub mod storage_types;

#cfg(test)]
mod test;

use crate::errors::Error;
use crate::storage_types::MintWrapRequest;

#[contract]
pub struct ProxyContract;

#[contractimpl]
impl ProxyContract {
    pub fn batch_mint_wrap(
        env: Env,
        wrap_contract: Address,
        requests: Vec<MintWrapRequest>,
    ) -> Result<(), Error> {
        if requests.is_empty() {
            return Err(Error::EmptyBatch);
        }
        for request in requests.iter() {
            call_mint_wrap(&env, &wrap_contract, &request);
        }
        events::emit_batch_mint_wrap(&env, &wrap_contract, &requests);
        Ok(())
    }
}

fn call_mint_wrap(env: &Env, wrap_contract: &Address, request: &MintWrapRequest) {
    let args: Vec<Val> = Vec::from_array(env, [request.public_key.clone().into_val(env), request.signature.clone().into_val(env), request.recipient.clone().into_val(env), request.amount.into_val(env)]);
    env.invoke_contract(wrap_contract, &Symbol::new(env, "mint_wrap"), args);
}
