// Engine tests for MilestoneEscrow.
//
// Covers the deal flow (fund, deliver, approve, cancel, refund), the
// deadline rules, the frozen-dispute rules, the terminal states, and the
// adapter interface (freeze_for_dispute, execute_ruling) against the real
// Ootle engine. No mocked engine, no network.

use ootle_byte_type::ToByteType;
use sha2::{Digest, Sha256};
use tari_crypto::ristretto::{RistrettoPublicKey, RistrettoSecretKey};
use tari_engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId};
use tari_ootle_transaction::args;
use tari_template_lib::types::{
    constants::TARI_TOKEN, crypto::RistrettoPublicKeyBytes, Amount, ComponentAddress, Hash32,
    NonFungibleAddress,
};
use tari_template_test_tooling::{support::assert_error::assert_reject_reason, TemplateTest};

const TOTAL: u64 = 500_000;
const FIRST: u64 = 200_000;
const SECOND: u64 = 300_000;
const AMOUNTS: [u64; 2] = [FIRST, SECOND];
const DEADLINE: u64 = 10;
const DRAW_DELAY: u64 = 2;
const JUROR_FEE: u64 = 5_000;
const BOND_FLOOR: u64 = 30_000;
const VOTE_PAY: u64 = 0;
const VOTE_REFUND: u64 = 1;

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
    TemplateTest::new("../..", ["templates/verdict", "templates/milestone-escrow"])
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
    amounts: Vec<u64>,
    deadline: u64,
    creator: &RistrettoSecretKey,
) -> ComponentAddress {
    let template = test.get_template_address("MilestoneEscrow");
    test.execute_expect_success(
        test.transaction()
            .call_function(
                template,
                ctor,
                args![arbitrator, payee_pk, amounts, deadline],
            )
            .build_and_seal(creator),
        vec![],
    );
    test.expect_return(0)
}

fn make_setup(ctor: &str) -> Setup {
    make_setup_with(ctor, AMOUNTS.to_vec(), DEADLINE)
}

fn make_setup_with(ctor: &str, amounts: Vec<u64>, deadline: u64) -> Setup {
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
        amounts,
        deadline,
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

fn deliver(setup: &mut Setup, index: u64) {
    let payee_secret = setup.payee.secret.clone();
    let escrow = setup.escrow;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "mark_delivered", args![index])
            .build_and_seal(&payee_secret),
        vec![],
    );
}

fn approve(setup: &mut Setup, index: u64) {
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let payee_account = setup.payee.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "approve", args![index, payee_account])
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

/// Settle a dispute through the court. The ruling is derived from the
/// revealed votes by the court, and the payout accounts are derived from
/// the escrow by the court, so the caller names nothing. Settle runs only
/// once the reveal window has passed, so the epoch moves to the reveal
/// deadline first.
fn settle(setup: &mut Setup, signer: &RistrettoSecretKey, dispute_id: u64) {
    let deadline: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_reveal_deadline",
        args![dispute_id],
        vec![],
    );
    set_epoch(&mut setup.test, deadline);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
            .build_and_seal(signer),
        vec![],
    );
}

fn set_epoch_hash(test: &mut TemplateTest, hash: Hash32) {
    test.set_virtual_substate(
        VirtualSubstateId::CurrentEpochHash,
        VirtualSubstate::CurrentEpochHash(hash),
    );
}

fn hash_of(byte: u8) -> Hash32 {
    Hash32::from([byte; 32])
}

fn register_juror(setup: &mut Setup, juror: &Party, amount: u64) {
    let verdict = setup.verdict;
    let juror_account = juror.account;
    let juror_secret = juror.secret.clone();
    let juror_proof = juror.proof.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(juror_account, "withdraw", args![TARI_TOKEN, amount])
            .put_last_instruction_output_on_workspace("bond")
            .call_method(verdict, "register_juror", args![Workspace("bond")])
            .build_and_seal(&juror_secret),
        vec![juror_proof],
    );
}

/// The vote commitment, computed the same way the template does:
/// SHA-256 of (vote, salt, dispute id, juror), concatenated in order.
fn vote_commitment(
    vote: u64,
    salt: Hash32,
    dispute_id: u64,
    juror: RistrettoPublicKeyBytes,
) -> Hash32 {
    let mut hasher = Sha256::new();
    hasher.update(vote.to_le_bytes());
    hasher.update(salt.as_ref());
    hasher.update(dispute_id.to_le_bytes());
    hasher.update(juror.as_ref());
    let out = hasher.finalize();
    Hash32::from(<[u8; 32]>::try_from(out.as_slice()).expect("sha256 output is 32 bytes"))
}

/// Register `count` jurors at the bond floor and return them.
fn register_jurors(setup: &mut Setup, count: usize) -> Vec<Party> {
    let jurors: Vec<Party> = (0..count).map(|_| Party::create(&mut setup.test)).collect();
    for juror in &jurors {
        register_juror(setup, juror, BOND_FLOOR);
    }
    jurors
}

/// Draw the panel and rule the dispute with the given votes, one per
/// panel juror in panel order. The jurors must already be registered,
/// and the dispute must already be open, so the frozen pool holds them.
fn rule_dispute(setup: &mut Setup, jurors: &[Party], dispute_id: u64, votes: [u64; 3]) {
    set_epoch(&mut setup.test, DRAW_DELAY);
    set_epoch_hash(&mut setup.test, hash_of(0xE2));
    let payer_secret = setup.payer.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(setup.verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let panel: Vec<RistrettoPublicKeyBytes> = (0..3)
        .map(|i| {
            setup.test.call_method(
                setup.verdict,
                "dispute_panel_member",
                args![dispute_id, i],
                vec![],
            )
        })
        .collect();
    for (i, key) in panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn juror not found");
        let juror_secret = juror.secret.clone();
        let salt = hash_of((i + 1) as u8);
        let commitment = vote_commitment(votes[i], salt, dispute_id, *key);
        setup.test.execute_expect_success(
            setup
                .test
                .transaction()
                .call_method(setup.verdict, "commit_vote", args![dispute_id, commitment])
                .build_and_seal(&juror_secret),
            vec![],
        );
        setup.test.execute_expect_success(
            setup
                .test
                .transaction()
                .call_method(
                    setup.verdict,
                    "reveal_vote",
                    args![dispute_id, votes[i], salt],
                )
                .build_and_seal(&juror_secret),
            vec![],
        );
    }
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
fn constructor_rejects_empty_milestones() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let template = test.get_template_address("MilestoneEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payee.pk_bytes(), Vec::<u64>::new(), DEADLINE],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "at least one milestone");
}

#[test]
fn constructor_rejects_non_positive_milestone_amount() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let template = test.get_template_address("MilestoneEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payee.pk_bytes(), vec![0u64, SECOND], DEADLINE],
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
    let template = test.get_template_address("MilestoneEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payer.pk_bytes(), AMOUNTS.to_vec(), DEADLINE],
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
    let template = test.get_template_address("MilestoneEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payee.pk_bytes(), AMOUNTS.to_vec(), 0u64],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "deadline must be after the current epoch");
}

#[test]
fn constructor_rejects_milestone_total_overflow() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let template = test.get_template_address("MilestoneEscrow");
    let reason = test.execute_expect_failure(
        test.transaction()
            .call_function(
                template,
                "new",
                args![verdict, payee.pk_bytes(), vec![u64::MAX, 1u64], DEADLINE],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "overflow");
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
            .call_method(stranger_account, "withdraw", args![TARI_TOKEN, FIRST])
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
    fund(&mut setup, TOTAL);
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
    assert_reject_reason(&reason, "funding exceeds the milestone total");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

#[test]
fn partial_funding_then_first_milestone_releases() {
    let mut setup = make_setup("new");
    fund(&mut setup, FIRST);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), FIRST);
    deliver(&mut setup, 0);
    let payee_before = balance(&mut setup.test, &setup.payee);
    approve(&mut setup, 0);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before + FIRST);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    // The second milestone is delivered but not funded, so approval fails.
    deliver(&mut setup, 1);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![1u64, payee_account],
    );
    assert_reject_reason(&reason, "not funded");
}

#[test]
fn insufficient_funding_blocks_approval() {
    let mut setup = make_setup("new");
    fund(&mut setup, 100_000);
    deliver(&mut setup, 0);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "not funded");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 100_000);
}

// ----- delivery and approval -----

#[test]
fn approve_before_delivery_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "not delivered");
}

#[test]
fn approve_only_payer_can_approve() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payee_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "only the payer can approve");
}

#[test]
fn mark_delivered_only_payee_can_mark() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "mark_delivered", args![0u64]);
    assert_reject_reason(&reason, "only the payee can mark");
}

#[test]
fn double_delivery_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    let payee_secret = setup.payee.secret.clone();
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![0u64]);
    assert_reject_reason(&reason, "already delivered");
}

#[test]
fn out_of_range_milestone_index_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![2u64]);
    assert_reject_reason(&reason, "out of range");
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![99u64, payee_account],
    );
    assert_reject_reason(&reason, "out of range");
}

#[test]
fn approve_releases_exact_slice() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payee_before = balance(&mut setup.test, &setup.payee);
    deliver(&mut setup, 0);
    approve(&mut setup, 0);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before + FIRST);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), SECOND);
    deliver(&mut setup, 1);
    approve(&mut setup, 1);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before + TOTAL);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    assert!(is_complete(&mut setup));
}

#[test]
fn second_milestone_approval_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    approve(&mut setup, 0);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "already released");
}

#[test]
fn full_completion_closes_agreement() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    approve(&mut setup, 0);
    deliver(&mut setup, 1);
    approve(&mut setup, 1);
    assert!(is_complete(&mut setup));
    // Every further state change is refused on a complete agreement.
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "complete");
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "complete");
    set_epoch(&mut setup.test, DEADLINE);
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "complete");
}

// ----- refund and cancel -----

#[test]
fn refund_before_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "after the deadline");
}

#[test]
fn refund_after_deadline_returns_unreleased() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    approve(&mut setup, 0);
    let payer_before = balance(&mut setup.test, &setup.payer);
    set_epoch(&mut setup.test, DEADLINE);
    refund(&mut setup);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + SECOND
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
    fund(&mut setup, TOTAL);
    set_epoch(&mut setup.test, DEADLINE);
    refund(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "refunded");
}

#[test]
fn refund_blocked_while_frozen() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payer_secret = setup.payer.secret.clone();
    let _jurors = register_jurors(&mut setup, 3);
    open_dispute(&mut setup, &payer_secret);
    assert!(is_frozen(&mut setup));
    set_epoch(&mut setup.test, DEADLINE);
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "frozen");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

#[test]
fn cancel_returns_unreleased_before_deadline() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payer_before = balance(&mut setup.test, &setup.payer);
    cancel(&mut setup);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before + TOTAL);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let cancelled: bool = setup
        .test
        .call_method(setup.escrow, "is_cancelled", args![], vec![]);
    assert!(cancelled);
}

#[test]
fn cancel_twice_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    cancel(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "cancelled");
}

#[test]
fn actions_after_cancel_fail() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    cancel(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![0u64]);
    assert_reject_reason(&reason, "cancelled");
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "cancelled");
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "cancelled");
}

#[test]
fn actions_after_refund_fail() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    set_epoch(&mut setup.test, DEADLINE);
    refund(&mut setup);
    let payer_secret = setup.payer.secret.clone();
    let payee_secret = setup.payee.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![0u64]);
    assert_reject_reason(&reason, "refunded");
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "refunded");
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "refunded");
}

#[test]
fn cancel_after_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "deadline");
}

#[test]
fn approve_after_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "deadline");
}

#[test]
fn mark_delivered_after_deadline_fails() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    set_epoch(&mut setup.test, DEADLINE);
    let payee_secret = setup.payee.secret.clone();
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![0u64]);
    assert_reject_reason(&reason, "deadline");
}

#[test]
fn fund_after_deadline_fails() {
    let mut setup = make_setup("new");
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let payer_proof = setup.payer.proof.clone();
    let payer_account = setup.payer.account;
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer_account, "withdraw", args![TARI_TOKEN, FIRST])
            .put_last_instruction_output_on_workspace("funding")
            .call_method(escrow, "fund", args![Workspace("funding")])
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
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
    fund(&mut setup, TOTAL);
    deliver(&mut setup, 0);
    approve(&mut setup, 0);
    deliver(&mut setup, 1);
    approve(&mut setup, 1);
    set_epoch(&mut setup.test, DEADLINE);
    let payer_secret = setup.payer.secret.clone();
    let reason = call_expect_failure(&mut setup, &payer_secret, "refund", args![]);
    assert_reject_reason(&reason, "complete");
}

// ----- freeze blocks state changes -----

#[test]
fn freeze_blocks_state_changes() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let payer_secret = setup.payer.secret.clone();
    let payee_secret = setup.payee.secret.clone();
    let _jurors = register_jurors(&mut setup, 3);
    open_dispute(&mut setup, &payer_secret);
    assert!(is_frozen(&mut setup));
    let payee_account = setup.payee.account;
    let reason = call_expect_failure(&mut setup, &payee_secret, "mark_delivered", args![0u64]);
    assert_reject_reason(&reason, "frozen");
    let reason = call_expect_failure(
        &mut setup,
        &payer_secret,
        "approve",
        args![0u64, payee_account],
    );
    assert_reject_reason(&reason, "frozen");
    let reason = call_expect_failure(&mut setup, &payer_secret, "cancel", args![]);
    assert_reject_reason(&reason, "frozen");
    let payer_proof = setup.payer.proof.clone();
    let payer_account = setup.payer.account;
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer_account, "withdraw", args![TARI_TOKEN, FIRST])
            .put_last_instruction_output_on_workspace("funding")
            .call_method(escrow, "fund", args![Workspace("funding")])
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "frozen");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

// ----- adapter -----

#[test]
fn adapter_freeze_and_execute_once() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    assert!(is_frozen(&mut setup));
    let payer_before = balance(&mut setup.test, &setup.payer);
    // Two jurors rule pay, one rules refund: the ruling is pay, the
    // two majority jurors are owed one fee each from the principal,
    // and the payer keeps nothing.
    rule_dispute(
        &mut setup,
        &jurors,
        dispute_id,
        [VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    );
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + TOTAL - 2 * JUROR_FEE
    );
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    assert!(!is_frozen(&mut setup));
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(executed);
    // Second settle of the same dispute fails at the court, so the ruling
    // executes exactly once.
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already settled");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
}

#[test]
fn adapter_stranger_freeze_rejected() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
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
    fund(&mut setup, TOTAL);
    let stranger_secret = setup.stranger.secret.clone();
    let reason = call_expect_failure(
        &mut setup,
        &stranger_secret,
        "execute_ruling",
        args![0u64, VOTE_PAY, (SECOND, FIRST)],
    );
    assert_reject_reason(
        &reason,
        "Access Denied: call component method 'execute_ruling'",
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

#[test]
fn adapter_execute_without_freeze_fails() {
    // The owned twin lets the owner call execute_ruling directly, so the
    // escrow's own frozen guard is what refuses a ruling on an unfrozen
    // escrow. (Through the court, a settle without an open dispute fails
    // in the court first, with "dispute id out of range".)
    let mut setup = make_setup("new_owned");
    fund(&mut setup, TOTAL);
    let payer_secret = setup.payer.secret.clone();
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                escrow,
                "execute_ruling",
                args![0u64, VOTE_PAY, (SECOND, FIRST)],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "not frozen");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

#[test]
fn adapter_execute_wrong_dispute_id_fails() {
    let mut setup = make_setup("new_owned");
    fund(&mut setup, TOTAL);
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
            .call_method(
                escrow,
                "execute_ruling",
                args![1u64, VOTE_PAY, (SECOND, FIRST)],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "does not match");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

#[test]
fn adapter_refund_returns_principal_less_juror_fees() {
    let mut setup = make_setup("new");
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    // Two jurors rule refund, one rules pay: the ruling is refund, the
    // principal returns to the payer less the two majority jurors' fees,
    // and nothing stays locked in the vault.
    rule_dispute(
        &mut setup,
        &jurors,
        dispute_id,
        [VOTE_REFUND, VOTE_REFUND, VOTE_PAY],
    );
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + TOTAL - 2 * JUROR_FEE
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let complete: bool = setup
        .test
        .call_method(setup.escrow, "is_complete", args![], vec![]);
    assert!(complete);
}

#[test]
fn adapter_execute_amounts_not_summing_fails() {
    // The owned twin lets the owner call execute_ruling directly, so the
    // escrow's own sum check is what refuses amounts that do not add up
    // to the vault balance.
    let mut setup = make_setup("new_owned");
    fund(&mut setup, TOTAL);
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
            .call_method(
                escrow,
                "execute_ruling",
                args![0u64, VOTE_PAY, (SECOND, FIRST - 1)],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "must equal the vault balance");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(!executed);
}

#[test]
fn adapter_execute_refund_nonzero_payee_fails() {
    // A refund pays the payee nothing: a refund ruling that pays the
    // payee is refused and leaves the escrow untouched.
    let mut setup = make_setup("new_owned");
    fund(&mut setup, TOTAL);
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
            .call_method(
                escrow,
                "execute_ruling",
                args![0u64, VOTE_REFUND, (SECOND, FIRST)],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "refund pays the payee nothing");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(!executed);
}

#[test]
fn adapter_execute_second_fails() {
    // One call returns both buckets and empties the vault, so a second
    // execute_ruling on the same dispute is refused. The first ruling
    // runs through the court, which deposits both buckets, so no bucket
    // is left dangling in the test transaction.
    let mut setup = make_setup("new_owned");
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    rule_dispute(
        &mut setup,
        &jurors,
        dispute_id,
        [VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    );
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(executed);
    let complete: bool = setup
        .test
        .call_method(setup.escrow, "is_complete", args![], vec![]);
    assert!(complete);
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                escrow,
                "execute_ruling",
                args![0u64, VOTE_PAY, (SECOND, FIRST)],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "ruling already executed");
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
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
    fund(&mut setup, TOTAL);
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
