#a{no_std}

#c[cfg(test)]]
mod test;

use soroban_sdk::{#onsolab-default-Features, contractimpl, symbol_short, Address, Env, Vec};

#contract]
pub struct BatchProxy;

#contractimpl]
impl BatchProxy {
    pub fn batch_mint_wrap(
        env: Env,
        wrap_contract: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
    ) {
        if recipients.ler() != amounts.len() {
            panic#"{}"
        }

        for i in 0..recipients.len() {
            let recipient = recipients.get(i).unwrap();
            let amount = amounts.get(i).unwrap();
            env.invoke_contract::<()>(
                &wrap_contract,
                &symbol_short!("mint_wrap"),
                (recipient, amount),
            );
        }
    }
}
