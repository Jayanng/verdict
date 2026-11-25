// Engine tests for PaymentEscrow.
//
// Covers the deal flow (fund, deliver, approve, cancel, refund), the
// deadline rules, the frozen-dispute rules, the terminal states, and the
// adapter interface (freeze_for_dispute, execute_ruling) against the real
// Ootle engine. No mocked engine, no network.

use ootle_byte_type::ToByteType;
use tari_crypto::ristretto::{RistrettoPublicKey, RistrettoSecretKey};
use tari_engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId};
use tari_ootle_transaction::args;
use tari_template_lib::types::{
    constants::TARI_TOKEN, crypto::RistrettoPublicKeyBytes, Amount, ComponentAddress,
    NonFungibleAddress,
};
use tari_template_test_tooling::{support::assert_error::assert_reject_reason, TemplateTest};

const AMOUNT: u64 = 500_000;
const DEADLINE: u64 = 10;
const DRAW_DELAY: u64 = 2;

struct Party {
    account: ComponentAddress,
    proof: NonFungibleAddress,
    secret: RistrettoSecretKey,
    public_key: RistrettoPublicKey,
}

impl Party {
    fn create(test: &mut TemplateTest) -> Self {
        let (account, proof, secret, public_key) = test.create_funded_account_with_keypair();
        Self {
            account,
            proof,
            secret,
            public_key,
        }
    }

    fn pk_bytes(&self) -> RistrettoPublicKeyBytes {
        self.public_key.to_byte_type()
    }
}

struct Setup {
    test: TemplateTest,
    verdict: ComponentAddress,
    escrow: ComponentAddress,
    payer: Party,
    payee: Party,
    stranger: Party,
}

fn new_test() -> TemplateTest {
    TemplateTest::new("../..", ["templates/verdict", "templates/payment-escrow"])
}

fn create_verdict(test: &mut TemplateTest, signer: &RistrettoSecretKey) -> ComponentAddress {
    let template = test.get_template_address("Verdict");
    test.execute_expect_success(
        test.transaction()
            .call_function(template, "new", args![])
            .build_and_seal(signer),
        vec![],
    );
    test.expect_return(0)
}

fn create_escrow(
    test: &mut TemplateTest,
    ctor: &str,
    arbitrator: ComponentAddress,
    payee_pk: RistrettoPublicKeyBytes,
    amount: u64,
    deadline: u64,
    creator: &RistrettoSecretKey,
) -> ComponentAddress {
    let template = test.get_template_address("PaymentEscrow");
    test.execute_expect_success(
        test.transaction()
            .call_function(
                template,
                ctor,
                args![arbitrator, payee_pk, amount, deadline],
            )
            .build_and_seal(creator),
        vec![],
    );
    test.expect_return(0)
}

fn make_setup(ctor: &str) -> Setup {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let stranger = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let escrow = create_escrow(
        &mut test,
        ctor,
        verdict,
        payee.pk_bytes(),
        AMOUNT,
        DEADLINE,
        &payer_secret,
    );
    Setup {
        test,
        verdict,
        escrow,
        payer,
        payee,
        stranger,
    }
}

fn balance(test: &mut TemplateTest, party: &Party) -> u64 {
    let amount: Amount = test.call_method(
        party.account,
        "balance",
        args![TARI_TOKEN],
        vec![party.proof.clone()],
    );
    amount
        .to_u64_checked()
        .expect("account balance out of range")
}

fn vault_balance(test: &mut TemplateTest, escrow: ComponentAddress) -> u64 {
    test.call_method(escrow, "vault_balance", args![], vec![])
}

fn fund(setup: &mut Setup, amount: u64) {
    let payer_account = setup.payer.account;
    let payer_secret = setup.payer.secret.clone();
    let payer_proof = setup.payer.proof.clone();
    let escrow = setup.escrow;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(payer_account, "withdraw", args![TARI_TOKEN, amount])
            .put_last_instruction_output_on_workspace("funding")
            .call_method(escrow, "fund", args![Workspace("funding")])
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
}

fn call_expect_failure(
    setup: &mut Setup,
    signer: &RistrettoSecretKey,
    method: &str,
    method_args: Vec<tari_ootle_transaction::builder::named_args::NamedArg>,
) -> tari_engine_types::commit_result::RejectReason {
    let escrow = setup.escrow;
    setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, method, method_args)
            .build_and_seal(signer),
        vec![],
    )
}

fn deliver(setup: &mut Setup) {
    let payee_secret = setup.payee.secret.clone();
    let escrow = setup.escrow;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "mark_delivered", args![])
            .build_and_seal(&payee_secret),
        vec![],
    );
}

fn approve(setup: &mut Setup) {
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let payee_account = setup.payee.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "approve", args![payee_account])
            .build_and_seal(&payer_secret),
        vec![],
    );
}

fn cancel(setup: &mut Setup) {
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let payer_account = setup.payer.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "cancel", args![])
            .put_last_instruction_output_on_workspace("cancelled")
            .call_method(payer_account, "deposit", args![Workspace("cancelled")])
            .build_and_seal(&payer_secret),
        vec![],
    );
}

fn refund(setup: &mut Setup) {
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let payer_account = setup.payer.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "refund", args![])
            .put_last_instruction_output_on_workspace("refunded")
            .call_method(payer_account, "deposit", args![Workspace("refunded")])
            .build_and_seal(&payer_secret),
        vec![],
    );
}

fn open_dispute(setup: &mut Setup, signer: &RistrettoSecretKey) -> u64 {
    let verdict = setup.verdict;
    let escrow = setup.escrow;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "open_dispute", args![escrow, DRAW_DELAY])
            .build_and_seal(signer),
        vec![],
    );
    setup.test.expect_return(0)
}

fn settle(setup: &mut Setup, signer: &RistrettoSecretKey, dispute_id: u64, payee_amount: u64) {
    let verdict = setup.verdict;
    let payee_account = setup.payee.account;
    let payer_account = setup.payer.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "settle",
                args![dispute_id, payee_amount, payee_account, payer_account],
            )
            .build_and_seal(signer),
        vec![],
    );
}

fn set_epoch(test: &mut TemplateTest, epoch: u64) {
    test.set_virtual_substate(
        VirtualSubstateId::CurrentEpoch,
        VirtualSubstate::CurrentEpoch(epoch),
    );
}

fn is_frozen(setup: &mut Setup) -> bool {
    setup
        .test
        .call_method(setup.escrow, "is_frozen", args![], vec![])
}

fn is_complete(setup: &mut Setup) -> bool {
    setup
        .test
        .call_method(setup.escrow, "is_complete", args![], vec![])
}

// ----- constructor -----

#[test]
fn constructor_rejects_non_positive_amount() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let template = test.get_template_address("PaymentEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payee.pk_bytes(), 0u64, DEADLINE],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "must be positive");
}

#[test]
fn constructor_rejects_payee_same_as_payer() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let template = test.get_template_address("PaymentEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payer.pk_bytes(), AMOUNT, DEADLINE],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "payee must differ from the payer");
}

#[test]
fn constructor_rejects_deadline_in_past() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let template = test.get_template_address("PaymentEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payee.pk_bytes(), AMOUNT, 0u64],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "deadline must be after the current epoch");
}

// ----- funding -----

#[test]
fn fund_only_payer_can_fund() {
    let mut setup = make_setup("new");
    let stranger_secret = setup.stranger.secret.clone();
    let stranger_proof = setup.stranger.proof.clone();
    let stranger_account = setup.stranger.account;
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(stranger_account, "withdraw", args![TARI_TOKEN, AMOUNT])
            .put_last_instruction_output_on_workspace("funding")
            .call_method(escrow, "fund", args![Workspace("funding")])
            .build_and_seal(&stranger_secret),
        vec![stranger_proof],
    );
    assert_reject_reason(&reason, "only the payer can fund");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
}

#[test]
fn fund_rejects_overfunding() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let payer_proof = setup.payer.proof.clone();
    let payer_account = setup.payer.account;
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer_account, "withdraw", args![TARI_TOKEN, 1u64])
            .put_last_instruction_output_on_workspace("funding")
            .call_method(escrow, "fund", args![Workspace("funding")])
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "funding exceeds the payment amount");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

#[test]
fn partial_funding_blocks_approval() {
    let mut setup = make_setup("new");
    fund(&mut setup, 100_000);
    deliver(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "not funded");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 100_000);
}

// ----- delivery and approval -----

#[test]
fn approve_before_delivery_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "not delivered");
}

#[test]
fn approve_only_payer_can_approve() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "only the payer can approve");
}

#[test]
fn mark_delivered_only_payee_can_mark() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "mark_delivered", args![]);
    assert_reject_reason(&reason, "only the payee can mark");
}

#[test]
fn double_delivery_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    let payee_secret = setup.payee.secret.clone();
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![]);
    assert_reject_reason(&reason, "already delivered");
}

#[test]
fn approve_releases_exact_amount() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    let payee_before = balance(&mut setup.test, &setup.payee);
    approve(&mut setup);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + AMOUNT
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    assert!(is_complete(&mut setup));
}

#[test]
fn second_approval_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    approve(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "already released");
}

#[test]
fn full_completion_closes_agreement() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    approve(&mut setup);
    assert!(is_complete(&mut setup));
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "already released");
    set_epoch(&mut setup.test, DEADLINE);
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "already released");
}

// ----- refund and cancel -----

#[test]
fn refund_before_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "after the deadline");
}

#[test]
fn refund_after_deadline_returns_funds() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_before = balance(&mut setup.test, &setup.payer);
    set_epoch(&mut setup.test, DEADLINE);
    refund(&mut setup);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + AMOUNT
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let refunded: bool = setup
        .test
        .call_method(setup.escrow, "is_refunded", args![], vec![]);
    assert!(refunded);
}

#[test]
fn refund_twice_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    set_epoch(&mut setup.test, DEADLINE);
    refund(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "refunded");
}

#[test]
fn refund_blocked_while_frozen() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    open_dispute(&mut setup, &payer_secret);
    assert!(is_frozen(&mut setup));
    set_epoch(&mut setup.test, DEADLINE);
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "frozen");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

#[test]
fn cancel_returns_funds_before_deadline() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_before = balance(&mut setup.test, &setup.payer);
    cancel(&mut setup);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + AMOUNT
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let cancelled: bool = setup
        .test
        .call_method(setup.escrow, "is_cancelled", args![], vec![]);
    assert!(cancelled);
}

#[test]
fn cancel_twice_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    cancel(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "cancelled");
}

#[test]
fn actions_after_cancel_fail() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    cancel(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![]);
    assert_reject_reason(&reason, "cancelled");
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "cancelled");
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "cancelled");
}

#[test]
fn actions_after_refund_fail() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    set_epoch(&mut setup.test, DEADLINE);
    refund(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![]);
    assert_reject_reason(&reason, "refunded");
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "refunded");
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "refunded");
}

#[test]
fn cancel_after_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "deadline");
}

#[test]
fn approve_after_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "deadline");
}

#[test]
fn mark_delivered_after_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    set_epoch(&mut setup.test, DEADLINE);
    let payee_secret = setup.payee.secret.clone();
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![]);
    assert_reject_reason(&reason, "deadline");
}

#[test]
fn cancel_empty_vault_succeeds() {
    let mut setup = make_setup("new");
    let payer_before = balance(&mut setup.test, &setup.payer);
    cancel(&mut setup);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let cancelled: bool = setup
        .test
        .call_method(setup.escrow, "is_cancelled", args![], vec![]);
    assert!(cancelled);
}

#[test]
fn refund_empty_vault_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    deliver(&mut setup);
    approve(&mut setup);
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "already released");
}

// ----- freeze blocks state changes -----

#[test]
fn freeze_blocks_state_changes() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let payee_secret = setup.payee.secret.clone();
    open_dispute(&mut setup, &payer_secret);
    assert!(is_frozen(&mut setup));
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![]);
    assert_reject_reason(&reason, "frozen");
    let reason = call_expect_failure(&mut setup, &payer_secret, "approve", args![payee_account]);
    assert_reject_reason(&reason, "frozen");
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "frozen");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

// ----- adapter -----

#[test]
fn adapter_freeze_and_execute_once() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    assert!(is_frozen(&mut setup));
    let payer_before = balance(&mut setup.test, &setup.payer);
    settle(&mut setup, &payer_secret, dispute_id, 300_000);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 300_000
    );
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + 200_000
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    assert!(!is_frozen(&mut setup));
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(executed);
    let verdict = setup.verdict;
    let payee_account = setup.payee.account;
    let payer_account = setup.payer.account;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "settle",
                args![dispute_id, 300_000u64, payee_account, payer_account],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already settled");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
}

#[test]
fn adapter_stranger_freeze_rejected() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let stranger_secret = setup.stranger.secret.clone();
    let reason = call_expect_failure(
        &mut setup,
        &stranger_secret,
        "freeze_for_dispute",
        args![0u64],
    );
    assert_reject_reason(
        &reason,
        "Access Denied: call component method 'freeze_for_dispute'",
    );
    assert!(!is_frozen(&mut setup));
}

#[test]
fn adapter_stranger_execute_rejected() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let stranger_secret = setup.stranger.secret.clone();
    let reason = call_expect_failure(
        &mut setup,
        &stranger_secret,
        "execute_ruling",
        args![0u64, 300_000u64],
    );
    assert_reject_reason(
        &reason,
        "Access Denied: call component method 'execute_ruling'",
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

#[test]
fn adapter_execute_without_freeze_fails() {
    // The owned twin lets the owner call execute_ruling directly, so the
    // escrow's own frozen guard is what refuses a ruling on an unfrozen
    // escrow. (Through the court, a settle without an open dispute fails
    // in the court first, with "dispute id out of range".)
    let mut setup = make_setup("new_owned");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "execute_ruling", args![0u64, 300_000u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "not frozen");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

#[test]
fn adapter_execute_wrong_dispute_id_fails() {
    let mut setup = make_setup("new_owned");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "execute_ruling", args![1u64, 300_000u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "does not match");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

#[test]
fn adapter_full_refund_returns_everything_to_payer() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    settle(&mut setup, &payer_secret, dispute_id, 0);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + AMOUNT
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let complete: bool = setup
        .test
        .call_method(setup.escrow, "is_complete", args![], vec![]);
    assert!(complete);
}

#[test]
fn adapter_withdraw_remainder_before_ruling_fails() {
    let mut setup = make_setup("new_owned");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "withdraw_remainder", args![])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "no ruling has executed");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), AMOUNT);
}

#[test]
fn adapter_withdraw_remainder_twice_fails() {
    let mut setup = make_setup("new_owned");
    fund(&mut setup, AMOUNT);
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let payer_account = setup.payer.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "execute_ruling", args![0u64, 300_000u64])
            .put_last_instruction_output_on_workspace("ruling")
            .call_method(setup.payee.account, "deposit", args![Workspace("ruling")])
            .build_and_seal(&payer_secret),
        vec![],
    );
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "withdraw_remainder", args![])
            .put_last_instruction_output_on_workspace("remainder")
            .call_method(payer_account, "deposit", args![Workspace("remainder")])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "withdraw_remainder", args![])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "remainder already withdrawn");
}

#[test]
fn adapter_freeze_empty_vault_fails() {
    let mut setup = make_setup("new");
    let payer_secret = setup.payer.secret.clone();
    let verdict = setup.verdict;
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "open_dispute", args![escrow, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "empty");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
}

#[test]
fn adapter_is_terminal_after_cancel_and_refund() {
    let mut setup = make_setup("new");
    fund(&mut setup, AMOUNT);
    let terminal: bool = setup
        .test
        .call_method(setup.escrow, "is_terminal", args![], vec![]);
    assert!(!terminal);
    cancel(&mut setup);
    let terminal: bool = setup
        .test
        .call_method(setup.escrow, "is_terminal", args![], vec![]);
    assert!(terminal);
    let complete: bool = setup
        .test
        .call_method(setup.escrow, "is_complete", args![], vec![]);
    assert!(!complete);
}
