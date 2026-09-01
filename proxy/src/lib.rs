#a[no_std]


#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, IntoVal, Vec};

#[contract]
pub struct BatchProxy;

#[contractimpl]
impl BatchProxy {
    pub fn batch_mint_wrap(
        env: Env,
        wrap_contract: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
    ) {
        if recipients.len() != amounts.len() {
            panic!("length mismatch");
        }

        for i in 0..recipients.len({
            let recipient = recipients.get(i).unwrap();
            let amount = amounts.get(i).unwrap();
            let args = (recipient, amount).into_val(&env);
            env.invoke_contract::<(>()>(&~wrap_contract, &symbol_short("mint_wrap"), args);
        }
    }
}
