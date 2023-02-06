import { expect } from "chai";
import fs from "fs";
import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { createAssociatedTokenAccountInstruction, createInitializeMintInstruction, createMintToInstruction } from "@solana/spl-token";
import { Mesh } from "../idl/mesh";

import { execSync } from "child_process";
import { LAMPORTS_PER_SOL, ParsedAccountData, PublicKey, SystemProgram } from "@solana/web3.js";

import BN from "bn.js";
import { ASSOCIATED_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@coral-xyz/anchor/dist/cjs/utils/token";

const deployMesh = () => {
  const deployCmd = `solana program deploy --url localhost -v --program-id $(pwd)/target/deploy/mesh-keypair.json $(pwd)/target/deploy/mesh.so`;
  execSync(deployCmd);
};

let provider;

describe("Programs", function(){

  this.beforeAll(function(){
    // Configure the client to use the local cluster.
    provider = anchor.AnchorProvider.env();
    anchor.setProvider(provider);
  });

    // test suite for the mesh program
  // TODO: UPDATE THIS
  describe.skip("Mesh Program", function(){
    let meshProgram;
    let ms;
    let members = [
      anchor.web3.Keypair.generate(),
      anchor.web3.Keypair.generate(),
      anchor.web3.Keypair.generate(),
    ];
    const createKey = anchor.web3.Keypair.generate().publicKey;

    this.beforeAll(async function(){
      deployMesh();
      console.log("✔ Mesh Program deployed.");
      meshProgram = anchor.workspace.Mesh as Program<Mesh>;
      [ms] = await getMsPDA(createKey, meshProgram.programId);
      await provider.connection.requestAirdrop(members[0].publicKey, anchor.web3.LAMPORTS_PER_SOL * 2);
      await provider.connection.requestAirdrop(members[1].publicKey, anchor.web3.LAMPORTS_PER_SOL * 2);
      await provider.connection.requestAirdrop(members[2].publicKey, anchor.web3.LAMPORTS_PER_SOL * 2);
    });

    it("Create a multisig", async function(){
        let initMembers = [
            members[0].publicKey,
            members[1].publicKey,
            members[2].publicKey
        ];
        try {
            await meshProgram.methods.create(provider.wallet.publicKey, 1, createKey, initMembers)
                .accounts({
                    multisig: ms,
                    creator: provider.wallet.publicKey
                })
                .rpc();
        }catch(e) {
            console.log(e);
        }

        const msState = await meshProgram.account.ms.fetch(ms);
        expect(msState.externalAuthority.toBase58()).to.equal(provider.wallet.publicKey.toBase58());
    });

    it("External remove", async function(){
        // get the state
        let msState = await meshProgram.account.ms.fetch(ms);
        const keyCount = (msState.keys as anchor.web3.PublicKey[]).length;
        // find a key to remove
        const removeKey = (msState.keys as anchor.web3.PublicKey[]).shift();
        try {
            await meshProgram.methods.removeMember(removeKey)
                .accounts({
                    multisig: ms,
                })
                .rpc();
        }catch(e){
            console.log(e);
        }
        msState = await meshProgram.account.ms.fetch(ms);
        expect((msState.keys as anchor.web3.PublicKey[]).length).to.equal(keyCount-1);
    });

    it("Internal remove failure", async function(){
        // get the state
        let msState = await meshProgram.account.ms.fetch(ms);
        const keyCount = (msState.keys as anchor.web3.PublicKey[]).length;
        // find a key to remove
        const removeKey = (msState.keys as anchor.web3.PublicKey[]).shift();
        const signerIndex = members.findIndex((k)=>{
            return k.publicKey.toBase58() === removeKey.toBase58();
        });
        const signer = members[signerIndex];

        const removeIx = await meshProgram.methods.removeMember(removeKey)
            .accounts({
                multisig: ms,
            })
            .instruction();
        try {
            const removeTx = new anchor.web3.Transaction();
            removeIx.keys.pop();
            removeIx.keys.push({
                pubkey: signer.publicKey,
                isWritable: true,
                isSigner: true,
            });
            removeTx.add(removeIx);
            await provider.sendAndConfirm(removeTx,[signer]);
        }catch(e){
            // want failure here
            // console.log(e);
        }
        msState = await meshProgram.account.ms.fetch(ms);
        expect((msState.keys as anchor.web3.PublicKey[]).length).to.equal(keyCount);
    });

    it("Vault withdrawal test - default authority", async function() {
        const msState = await meshProgram.account.ms.fetch(ms);
        const [vault] = await getAuthorityPDA(ms, new anchor.BN(1), meshProgram.programId);
        const withdrawIx = await createTestTransferTransaction(vault, provider.wallet.publicKey, anchor.web3.LAMPORTS_PER_SOL);
        const [tx] = await getTxPDA(ms, new anchor.BN(msState.transactionIndex + 1), meshProgram.programId);
        
        // go through the current member keys and find a signer
        const signerPubkey = (msState.keys as anchor.web3.PublicKey[])[0];
        const signerIndex = members.findIndex((k)=>{
            return k.publicKey.toBase58() === signerPubkey.toBase58();
        });

        const signer = members[signerIndex];

        // create the tx with authority 1
        try {
            await meshProgram.methods.createTransaction(1)
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc();
        }catch(e){
            console.log(e);
        }

        const [ix] = await getIxPDA(tx, new anchor.BN(1), meshProgram.programId);
        // add an instruction to use the default TX authority declared above
        try {
            await meshProgram.methods.addInstruction(withdrawIx, null, null, {default:{}})
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    instruction: ix,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc()
        }catch(e) {
            console.log(e);
        }

        // activate and approve
        try {
            await meshProgram.methods.activateTransaction()
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc();
            
            await meshProgram.methods.approveTransaction()
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    member: signer.publicKey
                })
                .signers([signer])
                .rpc();
        }catch(e) {
            console.log(e);
        }

        let txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executeReady");

        // airdrop 2 SOL to the vault
        try {
            const ad = await provider.connection.requestAirdrop(vault, anchor.web3.LAMPORTS_PER_SOL);
            await provider.connection.confirmTransaction(ad);
        }catch(e){
            console.log(e);
        }
        const vaultAccount = await provider.connection.getAccountInfo(vault);

        // execute the transaction
        try {
            await executeTransaction(tx, signer as unknown as anchor.Wallet, provider, meshProgram, signer.publicKey, [signer])
        }catch(e){
            console.log(e);
        }
        
        txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executed");
    });

    it("Vault withdrawal test - 2 different authorities", async function() {
        const msState = await meshProgram.account.ms.fetch(ms);
        const [vault1] = await getAuthorityPDA(ms, new anchor.BN(1), meshProgram.programId);
        const [vault2, vault2Bump] = await getAuthorityPDA(ms, new anchor.BN(2), meshProgram.programId);
        const withdrawIx1 = await createTestTransferTransaction(vault1, provider.wallet.publicKey, anchor.web3.LAMPORTS_PER_SOL);
        const withdrawIx2 = await createTestTransferTransaction(vault2, provider.wallet.publicKey, anchor.web3.LAMPORTS_PER_SOL);

        const [tx] = await getTxPDA(ms, new anchor.BN(msState.transactionIndex + 1), meshProgram.programId);
        
        // go through the current member keys and find a signer
        const signerPubkey = (msState.keys as anchor.web3.PublicKey[])[0];
        const signerIndex = members.findIndex((k)=>{
            return k.publicKey.toBase58() === signerPubkey.toBase58();
        });

        const signer = members[signerIndex];

        // transfer 2 SOL to the vaults
        const vault1TransferTx = new anchor.web3.Transaction();
        const vault2TransferTx = new anchor.web3.Transaction();
        const vault1TransferIx = await createTestTransferTransaction(provider.wallet.publicKey, vault1, anchor.web3.LAMPORTS_PER_SOL * 2);
        const vault2Transferix = await createTestTransferTransaction(provider.wallet.publicKey, vault2, anchor.web3.LAMPORTS_PER_SOL * 2);
        vault1TransferTx.add(vault1TransferIx);
        vault2TransferTx.add(vault2Transferix);
        
        await meshProgram.provider.sendAll([{tx: vault1TransferTx}, {tx: vault2TransferTx}]);
        
        // create the tx with authority 1
        try {
            await meshProgram.methods.createTransaction(1)
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc();
        }catch(e){
            console.log(e);
        }


        // add an instruction to use the default TX authority declared above
        const [ix1] = await getIxPDA(tx, new anchor.BN(1), meshProgram.programId);

        try {
            await meshProgram.methods.addInstruction(withdrawIx1, null, null, {default:{}})
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    instruction: ix1,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc()
        }catch(e) {
            console.log(e);
        }

        // add an instruction to use the default vault 2 TX authority declared above
        const [ix2] = await getIxPDA(tx, new anchor.BN(2), meshProgram.programId);
        try {
            await meshProgram.methods.addInstruction(withdrawIx2, 2, vault2Bump, {default:{}})
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    instruction: ix2,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc()
        }catch(e) {
            console.log(e);
        }

        // activate and approve
        try {
            await meshProgram.methods.activateTransaction()
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc();
            
            await meshProgram.methods.approveTransaction()
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    member: signer.publicKey
                })
                .signers([signer])
                .rpc();
        }catch(e) {
            console.log(e);
        }

        let txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executeReady");

        // make sure the vaults are funded
        const vaultStartBalance1 = await provider.connection.getAccountInfo(vault1, "confirmed");
        const vaultStartBalance2 = await provider.connection.getAccountInfo(vault2, "confirmed");
        
        expect(vaultStartBalance1.lamports).to.be.greaterThan(0);
        expect(vaultStartBalance2.lamports).to.be.greaterThan(0);
        // execute the transaction
        try {
            await executeTransaction(tx, signer as unknown as anchor.Wallet, provider, meshProgram, signer.publicKey, [signer])
        }catch(e){
            console.log(e);
        }
        
        const vaultEndBalance1 = await provider.connection.getAccountInfo(vault1);
        const vaultEndBalance2 =  await provider.connection.getAccountInfo(vault2);
        txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executed");

        expect(vaultEndBalance1.lamports).to.equal(vaultStartBalance1.lamports - anchor.web3.LAMPORTS_PER_SOL);
        expect(vaultEndBalance2.lamports).to.equal(vaultStartBalance2.lamports - anchor.web3.LAMPORTS_PER_SOL);
    });

    it("Transfer from vault to custom PDA, then transfer to vault 2", async function(){
        const msState = await meshProgram.account.ms.fetch(ms);
        // get next expected tx PDA
        const [tx] = await getTxPDA(ms, new anchor.BN(msState.transactionIndex + 1), meshProgram.programId);
        const [vault] = await getAuthorityPDA(ms, new anchor.BN(1), meshProgram.programId);
        const [vault2] = await getAuthorityPDA(ms, new anchor.BN(2), meshProgram.programId);
        // get the custom ix pda
        const [customIxPda, customIxPdaBump] = await getIxAuthority(tx, new anchor.BN(1), meshProgram.programId);
        // transfer from default vault to custom ix authority
        const withdrawIx = await createTestTransferTransaction(vault, customIxPda, anchor.web3.LAMPORTS_PER_SOL);
        // transfer from custom ix authority to vault 2
        const transferFromCustomIx = await createTestTransferTransaction(customIxPda, vault2, anchor.web3.LAMPORTS_PER_SOL);
        
        // go through the current member keys and find a signer
        const signerPubkey = (msState.keys as anchor.web3.PublicKey[])[0];
        const signerIndex = members.findIndex((k)=>{
            return k.publicKey.toBase58() === signerPubkey.toBase58();
        });

        const signer = members[signerIndex];

        // transfer 2 SOL to the vault
        const vaultTransferTx = new anchor.web3.Transaction();
        const vaultTransferIx = await createTestTransferTransaction(provider.wallet.publicKey, vault, anchor.web3.LAMPORTS_PER_SOL * 2);
        vaultTransferTx.add(vaultTransferIx);
        
        // vault 1 is funded
        await meshProgram.provider.sendAll([{tx: vaultTransferTx}]);

        // create the tx with authority 1 as default
        try {
            await meshProgram.methods.createTransaction(1)
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc();
        }catch(e){
            console.log(e);
        }


        // add an instruction to use the default TX authority declared above
        const [ix1] = await getIxPDA(tx, new anchor.BN(1), meshProgram.programId);
        // transfer from default vault to custom ix authority
        try {
            await meshProgram.methods.addInstruction(withdrawIx, null, null, {default:{}})
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    instruction: ix1,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc()
        }catch(e) {
            console.log(e);
        }

        // add instruction to transfer from the custom pda to vault 2
        const [ix2] = await getIxPDA(tx, new anchor.BN(2), meshProgram.programId);
        try {
            await meshProgram.methods.addInstruction(transferFromCustomIx, 1, customIxPdaBump, {custom:{}})
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    instruction: ix2,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc()
        }catch(e) {
            console.log(e);
        }

        // activate and approve
        try {
            await meshProgram.methods.activateTransaction()
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    creator: signer.publicKey
                })
                .signers([signer])
                .rpc();
            
            await meshProgram.methods.approveTransaction()
                .accounts({
                    multisig: ms,
                    transaction: tx,
                    member: signer.publicKey
                })
                .signers([signer])
                .rpc();
        }catch(e) {
            console.log(e);
        }

        let txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executeReady");

        // make sure the vaults are funded
        const vaultStartBalance1 = await provider.connection.getAccountInfo(vault, "confirmed");
        const vaultStartBalance2 = await provider.connection.getAccountInfo(vault2, "confirmed");
        expect(vaultStartBalance1.lamports).to.be.greaterThan(0);
        // execute the transaction
        try {
            await executeTransaction(tx, signer as unknown as anchor.Wallet, provider, meshProgram, signer.publicKey, [signer])
        }catch(e){
            console.log(e);
        }
        
        const vaultEndBalance1 = await provider.connection.getAccountInfo(vault);
        const vaultEndBalance2 =  await provider.connection.getAccountInfo(vault2);
        txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executed");

        expect(vaultEndBalance1.lamports).to.equal(vaultStartBalance1.lamports - anchor.web3.LAMPORTS_PER_SOL);
        expect(vaultEndBalance2.lamports).to.equal(vaultStartBalance2.lamports + anchor.web3.LAMPORTS_PER_SOL);
    });

    it("Create a token mint based off the custom ix PDA", async function(){
      // const tokenProgram = anchor.Spl.token(provider);
      // const ataProgram = anchor.Spl.associatedToken(provider);
      const mintAmount = 100000;
      const systemProgram = anchor.web3.SystemProgram;
      const msState = await meshProgram.account.ms.fetch(ms);
      // get next expected tx PDA
      const [tx] = await getTxPDA(ms, new anchor.BN(msState.transactionIndex + 1), meshProgram.programId);
      const [vault, vaultBump] = await getAuthorityPDA(ms, new anchor.BN(1), meshProgram.programId);
      const [vault2, vault2Bump] = await getAuthorityPDA(ms, new anchor.BN(2), meshProgram.programId);

      const [ix1] = await getIxPDA(tx, new anchor.BN(1), meshProgram.programId);
      const [ix2] = await getIxPDA(tx, new anchor.BN(2), meshProgram.programId);

      // go through the current member keys and find a signer
      const signerPubkey = (msState.keys as anchor.web3.PublicKey[])[0];
      const signerIndex = members.findIndex((k)=>{
          return k.publicKey.toBase58() === signerPubkey.toBase58();
      });

      const signer = members[signerIndex];

      // create the tx with authority 1 as default
      try {
        await meshProgram.methods.createTransaction(1)
            .accounts({
                multisig: ms,
                transaction: tx,
                creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
      }catch(e){
          console.log(e);
      }

      // use the tx custom authority to be the new mint account
      const [newMintPda, newMintPdaBump] = await getIxAuthority(tx, new anchor.BN(1), meshProgram.programId);
      const [vault1Ata] = await anchor.web3.PublicKey.findProgramAddress([
          vault.toBuffer(),
          TOKEN_PROGRAM_ID.toBuffer(),
          newMintPda.toBuffer()
        ], ASSOCIATED_PROGRAM_ID
      );
      const [vault2Ata] = await anchor.web3.PublicKey.findProgramAddress([
        vault2.toBuffer(),
        TOKEN_PROGRAM_ID.toBuffer(),
        newMintPda.toBuffer()
      ], ASSOCIATED_PROGRAM_ID
    );

      const createMintAccountIx = await systemProgram.createAccount({
          fromPubkey: vault,
          newAccountPubkey: newMintPda,
          lamports: await provider.connection.getMinimumBalanceForRentExemption(82),
          space: 82,
          programId: TOKEN_PROGRAM_ID
      });

      const initializeMintIx = await createInitializeMintInstruction(
        newMintPda,
          0,
          vault,
          null
        );

      // add the two mint instructions
        try {
          await meshProgram.methods.addInstruction(createMintAccountIx, 1, newMintPdaBump, {custom:{}})
              .accounts({
                  multisig: ms,
                  transaction: tx,
                  instruction: ix1,
                  creator: signer.publicKey
              })
              .signers([signer])
              .rpc();
        }catch(e){
            console.log(e);
        }

        try {
          await meshProgram.methods.addInstruction(initializeMintIx, 1, newMintPdaBump, {custom:{}})
            .accounts({
              multisig: ms,
              transaction: tx,
              instruction: ix2,
              creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }

        // mint to vault 1 instruction - with ata creation
        // mint to vault 2 instruction - with ata creation
        const [ix3] = await getIxPDA(tx, new anchor.BN(3), meshProgram.programId);
        const [ix4] = await getIxPDA(tx, new anchor.BN(4), meshProgram.programId);

        const vault1AtaIx = await createAssociatedTokenAccountInstruction(vault, vault1Ata,vault,newMintPda);

        try {
          await meshProgram.methods.addInstruction(vault1AtaIx, 1, vaultBump, {default:{}})
            .accounts({
              multisig: ms,
              transaction: tx,
              instruction: ix3,
              creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }

        const vault2AtaIx = await createAssociatedTokenAccountInstruction(vault, vault2Ata,vault,newMintPda);

        try {
          await meshProgram.methods.addInstruction(vault2AtaIx, 2, vault2Bump, {default:{}})
            .accounts({
              multisig: ms,
              transaction: tx,
              instruction: ix4,
              creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }

        // now add the mintTo to instructions for each ata
        const [ix5] = await getIxPDA(tx, new anchor.BN(5), meshProgram.programId);
        const [ix6] = await getIxPDA(tx, new anchor.BN(6), meshProgram.programId);
        const mintToVault1Ix = await createMintToInstruction(newMintPda, vault1Ata, vault, mintAmount);

        const mintToVault2Ix = await createMintToInstruction(newMintPda, vault2Ata, vault, mintAmount);

        // since the default TX authority is the vault1, and holds authority over the mint, these 2 ixes can use the default authority
        try {
          await meshProgram.methods.addInstruction(mintToVault1Ix, null, null, {default:{}})
            .accounts({
              multisig: ms,
              transaction: tx,
              instruction: ix5,
              creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }
        try {
          await meshProgram.methods.addInstruction(mintToVault2Ix, null, null, {default:{}})
            .accounts({
              multisig: ms,
              transaction: tx,
              instruction: ix6,
              creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }        


        // activate and approve
        try {
          await meshProgram.methods.activateTransaction()
            .accounts({
              multisig: ms,
              transaction: tx,
              creator: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }

        try {
          await meshProgram.methods.approveTransaction()
            .accounts({
              multisig: ms,
              transaction: tx,
              member: signer.publicKey
            })
            .signers([signer])
            .rpc();
        }catch(e){
            console.log(e);
        }

        let txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executeReady");

        // execute the transaction
        try {
          await executeTransaction(tx, signer as unknown as anchor.Wallet, provider, meshProgram, signer.publicKey, [signer])
        }catch(e){
          console.log(e);
        }
        txState = await meshProgram.account.msTransaction.fetch(tx);
        expect(txState.status).to.haveOwnProperty("executed");

        // check that the ATAs have the minted balances:
        const vault1AtaState = await provider.connection.getParsedAccountInfo(vault1Ata);
        const vault2AtaState = await provider.connection.getParsedAccountInfo(vault2Ata);
        expect(vault1AtaState.value.data.parsed.info.tokenAmount.uiAmount).to.equal(mintAmount);
        expect(vault2AtaState.value.data.parsed.info.tokenAmount.uiAmount).to.equal(mintAmount);
    });
  });

});
