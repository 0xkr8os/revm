use crate::{
    interpreter::{Gas, InstructionResult, SuccessOrHalt},
    primitives::{
        db::Database, EVMError, ExecutionResult, Output, ResultAndState, Spec, SpecId::LONDON, U256,
    },
    Context,
};

/// Mainnet end handle does not change the output.
#[inline]
pub fn end<EXT, DB: Database>(
    _context: &mut Context<EXT, DB>,
    evm_output: Result<ResultAndState, EVMError<DB::Error>>,
) -> Result<ResultAndState, EVMError<DB::Error>> {
    evm_output
}

/// Reward beneficiary with gas fee.
#[inline]
pub fn reward_beneficiary<SPEC: Spec, EXT, DB: Database>(
    context: &mut Context<EXT, DB>,
    gas: &Gas,
) -> Result<(), EVMError<DB::Error>> {
    let beneficiary = context.evm.env.block.coinbase;
    let effective_gas_price = context.evm.env.effective_gas_price();

    // transfer fee to coinbase/beneficiary.
    // EIP-1559 discard basefee for coinbase transfer. Basefee amount of gas is discarded.
    let coinbase_gas_price = if SPEC::enabled(LONDON) {
        effective_gas_price.saturating_sub(context.evm.env.block.basefee)
    } else {
        effective_gas_price
    };

    let (coinbase_account, _) = context
        .evm
        .journaled_state
        .load_account(beneficiary, &mut context.evm.db)
        .map_err(EVMError::Database)?;

    coinbase_account.mark_touch();
    coinbase_account.info.balance = coinbase_account
        .info
        .balance
        .saturating_add(coinbase_gas_price * U256::from(gas.spend() - gas.refunded() as u64));

    Ok(())
}

#[inline]
pub fn reimburse_caller<SPEC: Spec, EXT, DB: Database>(
    context: &mut Context<EXT, DB>,
    gas: &Gas,
) -> Result<(), EVMError<DB::Error>> {
    let caller = context.evm.env.tx.caller;
    let effective_gas_price = context.evm.env.effective_gas_price();

    // return balance of not spend gas.
    let (caller_account, _) = context
        .evm
        .journaled_state
        .load_account(caller, &mut context.evm.db)
        .map_err(EVMError::Database)?;

    caller_account.info.balance = caller_account
        .info
        .balance
        .saturating_add(effective_gas_price * U256::from(gas.remaining() + gas.refunded() as u64));

    Ok(())
}

/// Main return handle, returns the output of the transaction.
#[inline]
pub fn output<EXT, DB: Database>(
    context: &mut Context<EXT, DB>,
    call_result: InstructionResult,
    output: Output,
    gas: &Gas,
) -> Result<ResultAndState, EVMError<DB::Error>> {
    // used gas with refund calculated.
    let gas_refunded = gas.refunded() as u64;
    let final_gas_used = gas.spend() - gas_refunded;

    // reset journal and return present state.
    let (state, logs) = context.evm.journaled_state.finalize();

    let result = match call_result.into() {
        SuccessOrHalt::Success(reason) => ExecutionResult::Success {
            reason,
            gas_used: final_gas_used,
            gas_refunded,
            logs,
            output,
        },
        SuccessOrHalt::Revert => ExecutionResult::Revert {
            gas_used: final_gas_used,
            output: match output {
                Output::Call(return_value) => return_value,
                Output::Create(return_value, _) => return_value,
            },
        },
        SuccessOrHalt::Halt(reason) => ExecutionResult::Halt {
            reason,
            gas_used: final_gas_used,
        },
        SuccessOrHalt::FatalExternalError => {
            return Err(EVMError::Database(context.evm.error.take().unwrap()));
        }
        // Only two internal return flags.
        SuccessOrHalt::InternalContinue | SuccessOrHalt::InternalCallOrCreate => {
            panic!("Internal return flags should remain internal {call_result:?}")
        }
    };

    Ok(ResultAndState { result, state })
}
