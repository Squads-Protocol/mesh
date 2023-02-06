use anchor_lang::{prelude::*, solana_program::instruction::Instruction};

use state::mesh::*;
pub mod state;

use errors::*;
pub mod errors;

// INSERT PROGRAM ID
declare_id!("");

#[program]
pub mod mesh {

    use std::{convert::{TryInto}};

    use anchor_lang::solana_program::{program::{invoke_signed, invoke}, system_instruction::transfer};

    use super::*;
    
    // instruction to create a multisig
    pub fn create(ctx: Context<Create>, external_authority: Pubkey, threshold:u16, create_key: Pubkey, members: Vec<Pubkey>) -> Result<()> {
        // sort the members and remove duplicates
        let mut members = members;
        members.sort();
        members.dedup();

        // check we don't exceed u16
        let total_members = members.len();
        if total_members < 1 {
            return err!(GraphsError::EmptyMembers);
        }

        // make sure we don't exceed u16 on first call
        if total_members > usize::from(u16::MAX) {
            return err!(GraphsError::MaxMembersReached);
        }

        // make sure threshold is valid
        if usize::from(threshold) < 1 || usize::from(threshold) > total_members {
            return err!(GraphsError::InvalidThreshold);
        }

        ctx.accounts.multisig.init(
            external_authority,
            threshold,
            create_key,
            members,
            *ctx.bumps.get("multisig").unwrap(),
        )
    }

    // instruction to add a member/key to the multisig and reallocate space if neccessary
    pub fn add_member(ctx: Context<MsAuthRealloc>, new_member: Pubkey) -> Result<()> {
        // if max is already reached, we can't have more members
        if ctx.accounts.multisig.keys.len() >= usize::from(u16::MAX) {
            return err!(GraphsError::MaxMembersReached);
        }

        // check if realloc is needed
        let multisig_account_info = ctx.accounts.multisig.to_account_info();
        if *multisig_account_info.owner != mesh::ID {
            return err!(GraphsError::InvalidInstructionAccount);
        }
        let curr_data_size = multisig_account_info.data.borrow().len();
        let spots_left = ((curr_data_size - Ms::SIZE_WITHOUT_MEMBERS) / 32 ) - ctx.accounts.multisig.keys.len();

        // if not enough, add (10 * 32) to size - bump it up by 10 accounts
        if spots_left < 1 {
            // add space for 10 more keys
            let needed_len = curr_data_size + ( 10 * 32 );
            // reallocate more space
            AccountInfo::realloc(&multisig_account_info, needed_len, false)?;
            // if more lamports are needed, transfer them to the account
            let rent_exempt_lamports = ctx.accounts.rent.minimum_balance(needed_len).max(1);
            let top_up_lamports = rent_exempt_lamports.saturating_sub(ctx.accounts.multisig.to_account_info().lamports());
            if top_up_lamports > 0 {
                invoke(
                    &transfer(ctx.accounts.external_authority.key, &ctx.accounts.multisig.key(), top_up_lamports),
                    &[
                        ctx.accounts.external_authority.to_account_info().clone(),
                        multisig_account_info.clone(),
                        ctx.accounts.system_program.to_account_info().clone(),
                    ],
                )?;
            }
        }
        ctx.accounts.multisig.reload()?;
        ctx.accounts.multisig.add_member(new_member)?;
        let new_index = ctx.accounts.multisig.transaction_index;
        ctx.accounts.multisig.set_change_index(new_index)
    }

    // instruction to remove a member/key from the multisig
    pub fn remove_member(ctx: Context<MsAuth>, old_member: Pubkey) -> Result<()> {
        // if there is only one key in this multisig, reject the removal
        if ctx.accounts.multisig.keys.len() == 1 {
            return err!(GraphsError::CannotRemoveSoloMember);
        }
        ctx.accounts.multisig.remove_member(old_member)?;

        // if the number of keys is now less than the threshold, adjust it
        if ctx.accounts.multisig.keys.len() < usize::from(ctx.accounts.multisig.threshold) {
            let new_threshold: u16 = ctx.accounts.multisig.keys.len().try_into().unwrap();
            ctx.accounts.multisig.change_threshold(new_threshold)?;
        }
        let new_index = ctx.accounts.multisig.transaction_index;
        ctx.accounts.multisig.set_change_index(new_index)
    }

    // instruction to remove a member/key from the multisig and change the threshold
    pub fn remove_member_and_change_threshold<'info>(
        ctx: Context<'_,'_,'_,'info, MsAuth<'info>>, old_member: Pubkey, new_threshold: u16
    ) -> Result<()> {
        remove_member(
            Context::new(
                ctx.program_id,
                ctx.accounts,
                ctx.remaining_accounts,
                ctx.bumps.clone()
            ), old_member
        )?;
        change_threshold(ctx, new_threshold)
    }

    // instruction to add a member/key from the multisig and change the threshold
    pub fn add_member_and_change_threshold<'info>(
        ctx: Context<'_,'_,'_,'info, MsAuthRealloc<'info>>, new_member: Pubkey, new_threshold: u16
    ) -> Result<()> {
        // add the member
        add_member(
            Context::new(
                ctx.program_id,
                ctx.accounts,
                ctx.remaining_accounts,
                ctx.bumps.clone()
            ), new_member
        )?;

        // check that the threshold value is valid
        if ctx.accounts.multisig.keys.len() < usize::from(new_threshold) {
            let new_threshold: u16 = ctx.accounts.multisig.keys.len().try_into().unwrap();
            ctx.accounts.multisig.change_threshold(new_threshold)?;
        } else if new_threshold < 1 {
            return err!(GraphsError::InvalidThreshold);
        } else {
            ctx.accounts.multisig.change_threshold(new_threshold)?;
        }
        let new_index = ctx.accounts.multisig.transaction_index;
        ctx.accounts.multisig.set_change_index(new_index)
    }

    // instruction to change the threshold
    pub fn change_threshold(ctx: Context<MsAuth>, new_threshold: u16) -> Result<()> {
        // if the new threshold value is valid
        if ctx.accounts.multisig.keys.len() < usize::from(new_threshold) {
            let new_threshold: u16 = ctx.accounts.multisig.keys.len().try_into().unwrap();
            ctx.accounts.multisig.change_threshold(new_threshold)?;
        } else if new_threshold < 1 {
            return err!(GraphsError::InvalidThreshold);
        } else {
            ctx.accounts.multisig.change_threshold(new_threshold)?;
        }
        let new_index = ctx.accounts.multisig.transaction_index;
        ctx.accounts.multisig.set_change_index(new_index)
    }

    // instruction to increase the authority value tracked in the multisig
    // This is optional, as authorities are simply PDAs, however it may be helpful
    // to keep track of commonly used authorities in a UI.
    pub fn add_authority(ctx: Context<MsAuth>) -> Result<()> {
        ctx.accounts.multisig.add_authority()
    }

    // instruction to change the external execute setting, which allows
    // non-members or programs to execute a transaction.
    pub fn set_external_execute(ctx: Context<MsAuth>, setting: bool) -> Result<()> {
        let ms = &mut ctx.accounts.multisig;
        ms.allow_external_execute = setting;
        Ok(())
    }

    // instruction to create a transaction
    // each transaction is tied to a single authority, and must be specified when
    // creating the instruction below. authority 0 is reserved for internal
    // instructions, whereas authorities 1 or greater refer to a vault,
    // upgrade authority, or other.
    pub fn create_transaction(ctx: Context<CreateTransaction>, authority_index: u32) -> Result<()> {
        let ms = &mut ctx.accounts.multisig;
        let authority_bump =  {
                let (_, auth_bump) = Pubkey::find_program_address(&[
                    b"squad",
                    ms.key().as_ref(),
                    &authority_index.to_le_bytes(),
                    b"authority"
                ], ctx.program_id);
                auth_bump
        };

        ms.transaction_index =  ms.transaction_index.checked_add(1).unwrap();
        ctx.accounts.transaction.init(
            ctx.accounts.creator.key(),
            ms.key(),
            ms.transaction_index,
            *ctx.bumps.get("transaction").unwrap(),
            authority_index,
            authority_bump,
        )

    }

    // instruction to set the state of a transaction "active"
    // "active" transactions can then be signed off by multisig members
    pub fn activate_transaction(ctx: Context<ActivateTransaction>) -> Result<()> {
        ctx.accounts.transaction.activate()
    }

    // instruction to attach an instruction to a transaction
    // transactions must be in the "draft" status, and any
    // signer (aside from execution payer) must math the
    // authority specified during the transaction creation
    pub fn add_instruction(ctx: Context<AddInstruction>, incoming_instruction: IncomingInstruction, authority_index: Option<u32>, authority_bump: Option<u8>, authority_type: MsAuthorityType) -> Result<()> {
        let tx = &mut ctx.accounts.transaction;
        
        let mut ix_authority_index = authority_index;
        let mut ix_authority_bump = authority_bump;
        let mut ix_authority_type = authority_type;

        // check the proper authority level option is set
        if ix_authority_type != MsAuthorityType::Default && ix_authority_type != MsAuthorityType::Custom{
            return err!(GraphsError::InvalidAuthorityType);
        }

        // if no authority values are passed in, regardless of what the authority type is,
        // we will use the authority specified in the transaction and set the type to Default
        if authority_index.is_none() && authority_bump.is_none() {
            ix_authority_index = Some(tx.authority_index);
            ix_authority_bump = Some(tx.authority_bump);
            ix_authority_type = MsAuthorityType::Default;
        }

        // if one or the other is specified, throw an error
        if (authority_index.is_none() && authority_bump.is_some()) || (authority_index.is_some() && authority_bump.is_none()) {
            return err!(GraphsError::InvalidAuthorityIndex);
        }

        tx.instruction_index = tx.instruction_index.checked_add(1).unwrap();
        ctx.accounts.instruction.init(
            tx.instruction_index,
            incoming_instruction,
            *ctx.bumps.get("instruction").unwrap(),
            ix_authority_index,
            ix_authority_bump,
            ix_authority_type,
        )
    }

    // instruction to approve a transaction on behalf of a member
    // the transaction must have an "active" status
    pub fn approve_transaction(ctx: Context<VoteTransaction>) -> Result<()> {
        // if they have previously voted to reject, remove that item (change vote check)
        if let Some(ind) = ctx.accounts.transaction.has_voted_reject(ctx.accounts.member.key()) { ctx.accounts.transaction.remove_reject(ind)?; }

        // if they haven't already approved
        if ctx.accounts.transaction.has_voted_approve(ctx.accounts.member.key()).is_none() { ctx.accounts.transaction.sign(ctx.accounts.member.key())?; }

        // if current number of signers reaches threshold, mark the transaction as execute ready
        if ctx.accounts.transaction.approved.len() >= usize::from(ctx.accounts.multisig.threshold) {
            ctx.accounts.transaction.ready_to_execute()?;
        }
        Ok(())
    }

    // instruction to reject a transaction
    // the transaction must have an "active" status
    pub fn reject_transaction(ctx: Context<VoteTransaction>) -> Result<()> {
        // if they have previously voted to approve, remove that item (change vote check)
        if let Some(ind) = ctx.accounts.transaction.has_voted_approve(ctx.accounts.member.key()) { ctx.accounts.transaction.remove_approve(ind)?; }

        // check if they haven't already voted reject
        if ctx.accounts.transaction.has_voted_reject(ctx.accounts.member.key()).is_none() { ctx.accounts.transaction.reject(ctx.accounts.member.key())?; }

        // ie total members 7, threshold 3, cutoff = 4
        // ie total member 8, threshold 6, cutoff = 2
        let cutoff = ctx.accounts.multisig.keys.len().checked_sub(usize::from(ctx.accounts.multisig.threshold)).unwrap();
        if ctx.accounts.transaction.rejected.len() > cutoff {
            ctx.accounts.transaction.set_rejected()?;
        }
        Ok(())
    }

    // instruction to cancel a transaction
    // transactions must be in the "executeReady" status
    pub fn cancel_transaction(ctx: Context<CancelTransaction>) -> Result<()> {
        // check if they haven't cancelled yet
        if ctx.accounts.transaction.has_cancelled(ctx.accounts.member.key()).is_none() { ctx.accounts.transaction.cancel(ctx.accounts.member.key())? }

        // if the current number of signers reaches threshold, mark the transaction as "cancelled"
        if ctx.accounts.transaction.cancelled.len() >= usize::from(ctx.accounts.multisig.threshold) {
            ctx.accounts.transaction.set_cancelled()?;
        }
        Ok(())
    }

    // instruction to execute a transaction
    // transaction status must be "executeReady"
    pub fn execute_transaction<'info>(ctx: Context<'_,'_,'_,'info,ExecuteTransaction<'info>>, account_list: Vec<u8>) -> Result<()> {
        // check that we are provided at least one instruction
        if ctx.accounts.transaction.instruction_index < 1 {
            // if no instructions were found, mark it as executed and move on
            ctx.accounts.transaction.set_executed()?;
            return Ok(());
        }

        // use for derivation for the authority
        let ms_key = ctx.accounts.multisig.key();

        // unroll account infos from account_list
        let mapped_remaining_accounts: Vec<AccountInfo> = account_list.iter().map(|&i| {
            let index = usize::from(i);
            ctx.remaining_accounts[index].clone()
        }).collect();

        // iterator for remaining accounts
        let ix_iter = &mut mapped_remaining_accounts.iter();

        (1..=ctx.accounts.transaction.instruction_index).try_for_each(|i| {
            // each ix block starts with the ms_ix account
            let ms_ix_account: &AccountInfo = next_account_info(ix_iter)?;

            // if the attached instruction doesn't belong to this program, throw error
            if ms_ix_account.owner != ctx.program_id {
                return err!(GraphsError::InvalidInstructionAccount);
            }

            // deserialize the msIx
            let mut ix_account_data: &[u8] = &ms_ix_account.try_borrow_mut_data()?;
            let ms_ix: MsInstruction = MsInstruction::try_deserialize(&mut ix_account_data)?;

            // get the instruction account pda - seeded from transaction account + the transaction accounts instruction index
            let (ix_pda, _) = Pubkey::find_program_address(&[
                b"squad",
                ctx.accounts.transaction.key().as_ref(),
                &i.to_le_bytes(),
                b"instruction"],
                ctx.program_id
            );
            // check the instruction account key maches the derived pda
            if &ix_pda != ms_ix_account.key {
                return err!(GraphsError::InvalidInstructionAccount);
            }
            // get the instructions program account
            let ix_program_info: &AccountInfo = next_account_info(ix_iter)?;
            // check that it matches the submitted account
            if &ms_ix.program_id != ix_program_info.key {
                return err!(GraphsError::InvalidInstructionAccount);
            }

            let ix_keys = ms_ix.keys.clone();
            // create the instruction to invoke from the saved ms ix account
            let ix: Instruction = Instruction::from(ms_ix.clone());
            let mut ix_account_infos: Vec<AccountInfo> = Vec::<AccountInfo>::new();

            // add the program account needed for the ix
            ix_account_infos.push(ix_program_info.clone());

            // loop through the provided remaining accounts
            for ix_account in &ix_keys {
                let ix_account_info = next_account_info(ix_iter)?.clone();

                // check that the ix account keys match the submitted account keys
                if *ix_account_info.key != ix_account.pubkey {
                    return err!(GraphsError::InvalidInstructionAccount);
                }

                ix_account_infos.push(ix_account_info.clone());
            }

            let tx_key = ctx.accounts.transaction.key();
            let ms_ix_auth = ms_ix.clone();
            let authority_index = &ms_ix_auth.authority_index.unwrap().to_le_bytes();
            let authority_bump = ms_ix_auth.authority_bump.unwrap();

            // invoke based on whether the authority follows the default pda or custom ix level pda
            match ms_ix.authority_type {
                // invoke based on the default authority type
                MsAuthorityType::Default =>{
                    invoke_signed(
                        &ix,
                        &ix_account_infos,
                        &[&[
                            b"squad",
                            ms_key.as_ref(),
                            authority_index,
                            b"authority",
                            &[authority_bump]
                        ]]
                    )?
                },
                
                // invoke based on the custom pda & vault authority
                MsAuthorityType::Custom => {
                    invoke_signed(
                        &ix,
                        &ix_account_infos,
                        &[&[
                            b"squad",
                            tx_key.as_ref(),
                            authority_index,
                            b"ix_authority",
                            &[authority_bump],
                        ],
                        &[
                            b"squad",
                            ms_key.as_ref(),
                            &ctx.accounts.transaction.authority_index.to_le_bytes(),
                            b"authority",
                            &[ctx.accounts.transaction.authority_bump]
                        ]]
                    )?
                }
            };
 
            Ok(())
        })?;

        // mark it as executed
        ctx.accounts.transaction.set_executed()?;
        // reload any multisig changes
        ctx.accounts.multisig.reload()?;
        Ok(())
    }

    // instruction to sequentially execute parts of a transaction
    // instructions executed in this matter must be executed in order
    pub fn execute_instruction<'info>(ctx: Context<'_,'_,'_,'info,ExecuteInstruction<'info>>) -> Result<()> {
        let ms_key = &ctx.accounts.multisig.key();
        let ms_ix = &mut ctx.accounts.instruction;
        let tx = &mut ctx.accounts.transaction;

        // map the saved instruction account data to the instruction to be invoked
        let ix: Instruction = Instruction {
            accounts: ms_ix.keys.iter().map(|k| {
                AccountMeta {
                    pubkey: k.pubkey,
                    is_signer: k.is_signer,
                    is_writable:k.is_writable
                }
            }).collect(),
            data: ms_ix.data.clone(),
            program_id: ms_ix.program_id
        };

        // collect the accounts needed from remaining accounts (order matters)
        let mut ix_account_infos: Vec<AccountInfo> = Vec::<AccountInfo>::new();
        let ix_account_iter = &mut ctx.remaining_accounts.iter();
        // the first account in the submitted list should be the program
        let ix_program_account = next_account_info(ix_account_iter)?;
        // check that the programs match
        if ix_program_account.key != &ix.program_id {
            return err!(GraphsError::InvalidInstructionAccount);
        }

        // loop through the provided remaining accounts - check they match the saved instruction accounts
        for account_index in 0..ms_ix.keys.len() {
            let ix_account_info = next_account_info(ix_account_iter)?;
            // check that the ix account keys match the submitted account keys
            if ix_account_info.key != &ms_ix.keys[account_index].pubkey {
                return err!(GraphsError::InvalidInstructionAccount);
            }

            ix_account_infos.push(ix_account_info.clone());
        }

        let tx_key = tx.key();
        let ms_ix_auth = ms_ix.clone();
        let authority_index = &ms_ix_auth.authority_index.unwrap().to_le_bytes();
        let authority_bump = ms_ix_auth.authority_bump.unwrap();

        match ms_ix.authority_type {
            // invoke based on the default authority type
            MsAuthorityType::Default =>{
                invoke_signed(
                    &ix,
                    &ix_account_infos,
                    &[&[
                        b"squad",
                        ms_key.as_ref(),
                        authority_index,
                        b"authority",
                        &[authority_bump]
                    ]]
                )?
            },
            
            // invoke based on the custom pda
            MsAuthorityType::Custom => {
                invoke_signed(
                    &ix,
                    &ix_account_infos,
                    &[&[
                        b"squad",
                        tx_key.as_ref(),
                        authority_index,
                        b"ix_authority",
                        &[authority_bump],
                    ],
                    &[
                        b"squad",
                        ms_key.as_ref(),
                        &tx.authority_index.to_le_bytes(),
                        b"authority",
                        &[tx.authority_bump]
                    ]]
                )?
            }
        };

        // set the instruction as executed
        ms_ix.set_executed()?;
        // set the executed index to match
        tx.executed_index = ms_ix.instruction_index;
        // this is the last instruction - set the transaction as executed
        if ctx.accounts.instruction.instruction_index == ctx.accounts.transaction.instruction_index {
            ctx.accounts.transaction.set_executed()?;
        }
        // reload any multisig changes
        ctx.accounts.multisig.reload()?;
        Ok(())
    }

    // instruction to remove a member/key from the multisig and change the threshold
    pub fn change_external_authority<'info>(
        ctx: Context<MsAuth<'info>>, new_authority: Pubkey
    ) -> Result<()> {
        let ms = &mut ctx.accounts.multisig;
        ms.external_authority = new_authority;
        Ok(())
    }
    
}

#[derive(Accounts)]
#[instruction(external_authority: Pubkey, threshold: u16, create_key: Pubkey, members: Vec<Pubkey>)]
pub struct Create<'info> {
    #[account(
        init,
        payer = creator,
        space = Ms::SIZE_WITHOUT_MEMBERS + (members.len() * 32),
        seeds = [b"squad", create_key.as_ref(), b"multisig"], bump
    )]
    pub multisig: Account<'info, Ms>,

    #[account(mut)]
    pub creator: Signer<'info>,
    pub system_program: Program<'info, System>
}

#[derive(Accounts)]
pub struct CreateTransaction<'info> {
    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        constraint = multisig.is_member(creator.key()).is_some() @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Account<'info, Ms>,

    #[account(
        init,
        payer = creator,
        space = 8 + MsTransaction::initial_size_with_members(multisig.keys.len()),
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &multisig.transaction_index.checked_add(1).unwrap().to_le_bytes(),
            b"transaction"
        ], bump
    )]
    pub transaction: Account<'info, MsTransaction>,

    #[account(mut)]
    pub creator: Signer<'info>,
    pub system_program: Program<'info, System>
}

#[derive(Accounts)]
#[instruction(instruction_data: IncomingInstruction, authority_index: Option<u32>, authority_bump: Option<u8>, authority_type: MsAuthorityType)]
pub struct AddInstruction<'info> {
    #[account(
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        constraint = multisig.is_member(creator.key()).is_some() @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Account<'info, Ms>,

    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &transaction.transaction_index.to_le_bytes(),
            b"transaction"
        ], bump = transaction.bump,
        constraint = creator.key() == transaction.creator,
        constraint = transaction.status == MsTransactionStatus::Draft @GraphsError::InvalidTransactionState,
        constraint = transaction.ms == multisig.key() @GraphsError::InvalidInstructionAccount,
    )]
    pub transaction: Account<'info, MsTransaction>,

    #[account(
        init,
        payer = creator,
        space = 8 + instruction_data.get_max_size(),
        seeds = [
            b"squad",
            transaction.key().as_ref(),
            &transaction.instruction_index.checked_add(1).unwrap().to_le_bytes(),
            b"instruction"
        ], bump,
        constraint = 8 + instruction_data.get_max_size() <= MsInstruction::MAXIMUM_SIZE @GraphsError::InvalidTransactionState,
    )]
    pub instruction: Account<'info, MsInstruction>,

    #[account(mut)]
    pub creator: Signer<'info>,
    pub system_program: Program<'info, System>
}

#[derive(Accounts)]
pub struct ActivateTransaction<'info> {
    #[account(
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        constraint = multisig.is_member(creator.key()).is_some() @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Account<'info, Ms>,

    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &transaction.transaction_index.to_le_bytes(),
            b"transaction"
        ], bump = transaction.bump,
        constraint = creator.key() == transaction.creator,
        constraint = transaction.status == MsTransactionStatus::Draft @GraphsError::InvalidTransactionState,
        constraint = transaction.transaction_index > multisig.ms_change_index @GraphsError::DeprecatedTransaction,
        constraint = transaction.ms == multisig.key() @GraphsError::InvalidInstructionAccount,
    )]
    pub transaction: Account<'info, MsTransaction>,

    #[account(mut)]
    pub creator: Signer<'info>,
    pub system_program: Program<'info, System>
}

#[derive(Accounts)]
pub struct VoteTransaction<'info> {
    #[account(
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        constraint = multisig.is_member(member.key()).is_some() @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Account<'info, Ms>,

    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &transaction.transaction_index.to_le_bytes(),
            b"transaction"
        ], bump = transaction.bump,
        constraint = transaction.status == MsTransactionStatus::Active @GraphsError::InvalidTransactionState,
        constraint = transaction.transaction_index > multisig.ms_change_index @GraphsError::DeprecatedTransaction,
        constraint = transaction.ms == multisig.key() @GraphsError::InvalidInstructionAccount,
    )]
    pub transaction: Account<'info, MsTransaction>,

    #[account(mut)]
    pub member: Signer<'info>,
    pub system_program: Program<'info, System>
}

#[derive(Accounts)]
pub struct CancelTransaction<'info> {
    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        constraint = multisig.is_member(member.key()).is_some() @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Account<'info, Ms>,

    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &transaction.transaction_index.to_le_bytes(),
            b"transaction"
        ], bump = transaction.bump,
        constraint = transaction.status == MsTransactionStatus::ExecuteReady @GraphsError::InvalidTransactionState,
        constraint = transaction.ms == multisig.key() @GraphsError::InvalidInstructionAccount,
    )]
    pub transaction: Account<'info, MsTransaction>,

    #[account(mut)]
    pub member: Signer<'info>,
    pub system_program: Program<'info, System>
}

#[derive(Accounts)]
pub struct ExecuteTransaction<'info> {
    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        // only members can execute unless specified by the allow_external_execute setting
        constraint = multisig.is_member(member.key()).is_some() || multisig.allow_external_execute @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Box<Account<'info, Ms>>,

    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &transaction.transaction_index.to_le_bytes(),
            b"transaction"
        ], bump = transaction.bump,
        constraint = transaction.status == MsTransactionStatus::ExecuteReady @GraphsError::InvalidTransactionState,
        constraint = transaction.ms == multisig.key() @GraphsError::InvalidInstructionAccount,
        // if they've already started sequential execution, they must continue
        constraint = transaction.executed_index < 1 @GraphsError::PartialExecution,
    )]
    pub transaction: Account<'info, MsTransaction>,

    #[account(mut)]
    pub member: Signer<'info>,
}

// executes the the next instruction sequentially if a tx is executeReady
#[derive(Accounts)]
pub struct ExecuteInstruction<'info> {
    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ],
        bump = multisig.bump,
        constraint = multisig.is_member(member.key()).is_some() || multisig.allow_external_execute @GraphsError::KeyNotInMultisig,
    )]
    pub multisig: Box<Account<'info, Ms>>,

    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.key().as_ref(),
            &transaction.transaction_index.to_le_bytes(),
            b"transaction"
        ], bump = transaction.bump,
        constraint = transaction.status == MsTransactionStatus::ExecuteReady @GraphsError::InvalidTransactionState,
        constraint = transaction.ms == multisig.key() @GraphsError::InvalidInstructionAccount,
    )]
    pub transaction: Account<'info, MsTransaction>,
    
    #[account(
        mut,
        seeds = [
            b"squad",
            transaction.key().as_ref(),
            &transaction.executed_index.checked_add(1).unwrap().to_le_bytes(),
            b"instruction"
        ], bump = instruction.bump,
        constraint = !instruction.executed @GraphsError::InvalidInstructionAccount,
        // it should be the next expected instruction account to be executed
        constraint = instruction.instruction_index == transaction.executed_index.checked_add(1).unwrap() @GraphsError::InvalidInstructionAccount,
    )]
    pub instruction: Account<'info, MsInstruction>,

    #[account(mut)]
    pub member: Signer<'info>,
}

#[derive(Accounts)]
pub struct MsAuth<'info> {
    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ], bump = multisig.bump,
        constraint = multisig.external_authority == external_authority.key() @GraphsError::InvalidExternalAuthority
    )]
    multisig: Box<Account<'info, Ms>>,
    #[account(mut)]
    pub external_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct MsAuthRealloc<'info> {
    #[account(
        mut,
        seeds = [
            b"squad",
            multisig.create_key.as_ref(),
            b"multisig"
        ], bump = multisig.bump,
        constraint = multisig.external_authority == external_authority.key() @GraphsError::InvalidExternalAuthority
    )]
    multisig: Box<Account<'info, Ms>>,
    #[account(mut)]
    pub external_authority: Signer<'info>,
    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>
}
