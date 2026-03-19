use super::*;

/// Attempt a flash loan that references the same bank/vault twice in the loans
/// vector. The program should reject duplicate bank entries.
#[tokio::test]
#[ignore] // Requires serum_dex BPF program which is not available in the test environment
async fn test_flash_loan_security_duplicate_bank() -> Result<(), BanksClientError> {
    let mut test_builder = TestContextBuilder::new();
    test_builder.test().set_compute_max_units(100_000);
    let context = test_builder.start_default().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..2];

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;
    let bank = tokens[0].bank;

    // Provide liquidity so flash loans can work
    let provided_amount = 1000;
    create_funded_account(
        &solana,
        group,
        owner,
        1,
        &context.users[1],
        mints,
        provided_amount,
        0,
    )
    .await;

    // Create user account with some funds
    let deposit_amount = 100;
    let account = create_funded_account(
        &solana,
        group,
        owner,
        0,
        &context.users[1],
        &mints[..1],
        deposit_amount,
        0,
    )
    .await;

    let target_token_account = context.users[0].token_accounts[0];

    // Try a flash loan with the same bank listed twice
    let mut tx = ClientTransaction::new(solana);
    let loans = vec![
        FlashLoanPart {
            bank,
            token_account: target_token_account,
            withdraw_amount: 10,
        },
        FlashLoanPart {
            bank, // duplicate!
            token_account: target_token_account,
            withdraw_amount: 5,
        },
    ];
    tx.add_instruction(FlashLoanBeginInstruction {
        account,
        owner,
        loans: loans.clone(),
    })
    .await;
    tx.add_instruction(FlashLoanEndInstruction {
        account,
        owner,
        loans,
        flash_loan_type: mango_v4::accounts_ix::FlashLoanType::Unknown,
    })
    .await;

    let result = tx.send().await;
    assert!(
        result.is_err(),
        "flash loan with duplicate bank should be rejected"
    );

    Ok(())
}

/// Send flash_loan_begin without a corresponding flash_loan_end in the same
/// transaction. The transaction should fail because the program requires both
/// begin and end to be present.
#[tokio::test]
#[ignore] // Requires serum_dex BPF program which is not available in the test environment
async fn test_flash_loan_security_begin_end_mismatch() -> Result<(), BanksClientError> {
    let mut test_builder = TestContextBuilder::new();
    test_builder.test().set_compute_max_units(100_000);
    let context = test_builder.start_default().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..2];

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;
    let bank = tokens[0].bank;

    // Provide liquidity
    let provided_amount = 1000;
    create_funded_account(
        &solana,
        group,
        owner,
        1,
        &context.users[1],
        mints,
        provided_amount,
        0,
    )
    .await;

    // Create user account
    let deposit_amount = 100;
    let account = create_funded_account(
        &solana,
        group,
        owner,
        0,
        &context.users[1],
        &mints[..1],
        deposit_amount,
        0,
    )
    .await;

    let target_token_account = context.users[0].token_accounts[0];

    // Send only flash_loan_begin without flash_loan_end
    let mut tx = ClientTransaction::new(solana);
    let loans = vec![FlashLoanPart {
        bank,
        token_account: target_token_account,
        withdraw_amount: 10,
    }];
    tx.add_instruction(FlashLoanBeginInstruction {
        account,
        owner,
        loans,
    })
    .await;
    // Intentionally omit FlashLoanEndInstruction

    let result = tx.send().await;
    assert!(
        result.is_err(),
        "flash_loan_begin without flash_loan_end should fail"
    );

    Ok(())
}

/// Complete a flash loan but repay less than borrowed. The health check at
/// the end should detect the shortfall and fail the transaction.
#[tokio::test]
#[ignore] // Requires serum_dex BPF program which is not available in the test environment
async fn test_flash_loan_security_underpayment() -> Result<(), BanksClientError> {
    let mut test_builder = TestContextBuilder::new();
    test_builder.test().set_compute_max_units(100_000);
    let context = test_builder.start_default().await;
    let solana = &context.solana.clone();

    let admin = TestKeypair::new();
    let owner = context.users[0].key;
    let payer = context.users[1].key;
    let mints = &context.mints[0..2];

    let GroupWithTokens { group, tokens, .. } = GroupWithTokensConfig {
        admin,
        payer,
        mints: mints.to_vec(),
        ..GroupWithTokensConfig::default()
    }
    .create(solana)
    .await;
    let bank = tokens[0].bank;

    // Provide liquidity
    let provided_amount = 1000;
    create_funded_account(
        &solana,
        group,
        owner,
        1,
        &context.users[1],
        mints,
        provided_amount,
        0,
    )
    .await;

    // Create user account with a small deposit (not enough to cover shortfall)
    let deposit_amount = 10;
    let account = create_funded_account(
        &solana,
        group,
        owner,
        0,
        &context.users[1],
        &mints[..1],
        deposit_amount,
        0,
    )
    .await;

    let target_token_account = context.users[0].token_accounts[0];
    let margin_account = context.users[1].token_accounts[0];

    // Borrow a large amount but repay nothing — should fail health
    let withdraw_amount = 500u64;
    let deposit_amount_back = 0u64;

    let mut tx = ClientTransaction::new(solana);
    let loans = vec![FlashLoanPart {
        bank,
        token_account: target_token_account,
        withdraw_amount,
    }];
    tx.add_instruction(FlashLoanBeginInstruction {
        account,
        owner,
        loans: loans.clone(),
    })
    .await;
    // Transfer the borrowed funds away (simulating not repaying)
    if withdraw_amount > 0 {
        tx.add_instruction_direct(
            spl_token::instruction::transfer(
                &spl_token::ID,
                &target_token_account,
                &margin_account,
                &owner.pubkey(),
                &[&owner.pubkey()],
                withdraw_amount,
            )
            .unwrap(),
        );
    }
    tx.add_instruction(FlashLoanEndInstruction {
        account,
        owner,
        loans,
        flash_loan_type: mango_v4::accounts_ix::FlashLoanType::Unknown,
    })
    .await;

    let result = tx.send().await;
    assert!(
        result.is_err(),
        "flash loan with underpayment should fail health check"
    );

    Ok(())
}
