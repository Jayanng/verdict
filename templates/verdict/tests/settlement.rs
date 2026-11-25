// Engine tests for Phase 3: the settlement matrix, juror fees and
// slashing, the appeal path, the replacement draw, and the bounded
// no-ruling fallback.
//
// Every test runs against the real engine. The ruling is always derived
// from the revealed majority votes by the court, never supplied by the
// caller. Every settlement path asserts exact micro-Tari conservation:
// payee gain plus payer gain plus juror fees equals the principal.

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
const DRAW_DELAY: u64 = 2;
const MILESTONE_AMOUNTS: [u64; 2] = [200_000, 300_000];
const DEADLINE: u64 = 100;
const JUROR_FEE: u64 = 5_000;
const BOND_FLOOR: u64 = 30_000;
const APPEAL_FEE: u64 = 35_000;
const VOTE_PAY: u64 = 0;
const VOTE_REFUND: u64 = 1;
const VOTE_SPLIT: u64 = 2;
const VOTE_REFUSE: u64 = 3;
const NO_OUTCOME: u64 = 4;

#[derive(Clone)]
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

fn new_payment_test() -> TemplateTest {
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

fn create_milestone_escrow(
    test: &mut TemplateTest,
    arbitrator: ComponentAddress,
    payee_pk: RistrettoPublicKeyBytes,
    creator: &RistrettoSecretKey,
) -> ComponentAddress {
    let template = test.get_template_address("MilestoneEscrow");
    test.execute_expect_success(
        test.transaction()
            .call_function(
                template,
                "new",
                args![arbitrator, payee_pk, MILESTONE_AMOUNTS.to_vec(), DEADLINE],
            )
            .build_and_seal(creator),
        vec![],
    );
    test.expect_return(0)
}

fn create_payment_escrow(
    test: &mut TemplateTest,
    arbitrator: ComponentAddress,
    payee_pk: RistrettoPublicKeyBytes,
    amount: u64,
    creator: &RistrettoSecretKey,
) -> ComponentAddress {
    let template = test.get_template_address("PaymentEscrow");
    test.execute_expect_success(
        test.transaction()
            .call_function(
                template,
                "new",
                args![arbitrator, payee_pk, amount, DEADLINE],
            )
            .build_and_seal(creator),
        vec![],
    );
    test.expect_return(0)
}

fn make_setup() -> Setup {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let stranger = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let escrow = create_milestone_escrow(&mut test, verdict, payee.pk_bytes(), &payer_secret);
    Setup {
        test,
        verdict,
        escrow,
        payer,
        payee,
        stranger,
    }
}

fn make_payment_setup(amount: u64) -> Setup {
    let mut test = new_payment_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let stranger = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let escrow = create_payment_escrow(&mut test, verdict, payee.pk_bytes(), amount, &payer_secret);
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

fn vault_balance(setup: &mut Setup) -> u64 {
    setup
        .test
        .call_method(setup.escrow, "vault_balance", args![], vec![])
}

fn fund(setup: &mut Setup, amount: u64) {
    let payer_account = setup.payer.account;
    let payer_secret = setup.payer.secret.clone();
    let payer_proof = setup.payer.proof.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(payer_account, "withdraw", args![TARI_TOKEN, amount])
            .put_last_instruction_output_on_workspace("funding")
            .call_method(setup.escrow, "fund", args![Workspace("funding")])
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
}

fn open_dispute(setup: &mut Setup, signer: &RistrettoSecretKey) -> u64 {
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "open_dispute", args![setup.escrow, DRAW_DELAY])
            .build_and_seal(signer),
        vec![],
    );
    setup.test.expect_return(0)
}

fn set_epoch(test: &mut TemplateTest, epoch: u64) {
    test.set_virtual_substate(
        VirtualSubstateId::CurrentEpoch,
        VirtualSubstate::CurrentEpoch(epoch),
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

fn register_jurors(setup: &mut Setup, count: usize) -> Vec<Party> {
    let jurors: Vec<Party> = (0..count).map(|_| Party::create(&mut setup.test)).collect();
    for juror in &jurors {
        register_juror(setup, juror, BOND_FLOOR);
    }
    jurors
}

fn juror_bond(setup: &mut Setup, juror: RistrettoPublicKeyBytes) -> u64 {
    setup
        .test
        .call_method(setup.verdict, "juror_bond", args![juror], vec![])
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

fn panel_keys(setup: &mut Setup, dispute_id: u64) -> Vec<RistrettoPublicKeyBytes> {
    let len: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_panel_len",
        args![dispute_id],
        vec![],
    );
    (0..len)
        .map(|i| {
            setup.test.call_method(
                setup.verdict,
                "dispute_panel_member",
                args![dispute_id, i],
                vec![],
            )
        })
        .collect()
}

fn appeal_panel_keys(setup: &mut Setup, dispute_id: u64) -> Vec<RistrettoPublicKeyBytes> {
    let len: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_appeal_panel_len",
        args![dispute_id],
        vec![],
    );
    (0..len)
        .map(|i| {
            setup.test.call_method(
                setup.verdict,
                "dispute_appeal_panel_member",
                args![dispute_id, i],
                vec![],
            )
        })
        .collect()
}

fn draw_panel(setup: &mut Setup, dispute_id: u64) -> Vec<RistrettoPublicKeyBytes> {
    let payer_secret = setup.payer.secret.clone();
    set_epoch(&mut setup.test, DRAW_DELAY);
    set_epoch_hash(&mut setup.test, hash_of(0xE2));
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(setup.verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    panel_keys(setup, dispute_id)
}

fn commit_vote(setup: &mut Setup, juror: &Party, dispute_id: u64, commitment: Hash32) {
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&juror_secret),
        vec![],
    );
}

fn reveal_vote(setup: &mut Setup, juror: &Party, dispute_id: u64, vote: u64, salt: Hash32) {
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "reveal_vote", args![dispute_id, vote, salt])
            .build_and_seal(&juror_secret),
        vec![],
    );
}

/// Rule a dispute with the given votes, one per panel juror in panel
/// order. The jurors list must hold every drawn juror.
fn rule_panel(setup: &mut Setup, jurors: &[Party], dispute_id: u64, votes: &[u64]) {
    let panel = panel_keys(setup, dispute_id);
    assert_eq!(panel.len(), votes.len(), "votes must cover the panel");
    for (i, key) in panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn juror not found");
        let salt = hash_of((i + 1) as u8);
        let commitment = vote_commitment(votes[i], salt, dispute_id, *key);
        commit_vote(setup, juror, dispute_id, commitment);
        reveal_vote(setup, juror, dispute_id, votes[i], salt);
    }
}

/// Open a dispute, draw the panel, and rule it with the given votes.
/// Returns the dispute id and the drawn panel. The pool must already
/// hold exactly the jurors in `jurors`, so the panel is the whole pool.
fn dispute_with_ruling(
    setup: &mut Setup,
    jurors: &[Party],
    votes: &[u64],
) -> (u64, Vec<RistrettoPublicKeyBytes>) {
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(setup, &payer_secret);
    let panel = draw_panel(setup, dispute_id);
    rule_panel(setup, jurors, dispute_id, votes);
    (dispute_id, panel)
}

fn settle(setup: &mut Setup, signer: &RistrettoSecretKey, dispute_id: u64) {
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
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(signer),
        vec![],
    );
}

fn dispute_outcome(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup
        .test
        .call_method(setup.verdict, "dispute_outcome", args![dispute_id], vec![])
}

fn dispute_payee_amount(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_payee_amount",
        args![dispute_id],
        vec![],
    )
}

fn dispute_fees_paid(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_fees_paid",
        args![dispute_id],
        vec![],
    )
}

fn dispute_claim(setup: &mut Setup, dispute_id: u64, juror: RistrettoPublicKeyBytes) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_claim",
        args![dispute_id, juror],
        vec![],
    )
}

fn claim_fee(setup: &mut Setup, juror: &Party, dispute_id: u64) {
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "claim_fee", args![dispute_id, juror.account])
            .build_and_seal(&juror_secret),
        vec![],
    );
}

fn close_no_ruling(setup: &mut Setup, signer: &RistrettoSecretKey, dispute_id: u64) {
    let verdict = setup.verdict;
    let payee_account = setup.payee.account;
    let payer_account = setup.payer.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "close_no_ruling",
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(signer),
        vec![],
    );
}

// ----- matrix: pay -----

#[test]
fn settle_pay_majority_pays_fees_first() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);

    // Two majority jurors are owed one fee each: 10,000 of the 500,000
    // principal. The payee keeps 490,000, the payer keeps nothing.
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_PAY);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 490_000);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 10_000);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 490_000
    );
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before);
    assert_eq!(vault_balance(&mut setup), 0);
    // Conservation: payee 490,000 + payer 0 + fees 10,000 == 500,000.
    // The minority juror is slashed one fee of bond, and the forfeited
    // 5,000 tops up the two majority jurors by 2,500 each.
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[0]), 7_500);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[1]), 7_500);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[2]), 0);
    assert_eq!(juror_bond(&mut setup, panel[0]), BOND_FLOOR);
    assert_eq!(juror_bond(&mut setup, panel[1]), BOND_FLOOR);
    assert_eq!(juror_bond(&mut setup, panel[2]), BOND_FLOOR - JUROR_FEE);
    let settled: bool =
        setup
            .test
            .call_method(setup.verdict, "dispute_settled", args![dispute_id], vec![]);
    assert!(settled);
}

// ----- matrix: refund -----

#[test]
fn settle_refund_majority_returns_principal_less_fees() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_REFUND, VOTE_REFUND, VOTE_PAY]);
    settle(&mut setup, &payer_secret, dispute_id);

    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_REFUND);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 0);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 10_000);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + 490_000
    );
    assert_eq!(vault_balance(&mut setup), 0);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[0]), 7_500);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[1]), 7_500);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[2]), 0);
}

// ----- matrix: split -----

#[test]
fn settle_split_divides_post_fee_principal() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_SPLIT, VOTE_SPLIT, VOTE_PAY]);
    settle(&mut setup, &payer_secret, dispute_id);

    // Post-fee principal is 490,000; split halves it exactly.
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_SPLIT);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 245_000);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 10_000);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 245_000
    );
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + 245_000
    );
    assert_eq!(vault_balance(&mut setup), 0);
}

#[test]
fn settle_split_odd_micro_tari_goes_to_payee() {
    let mut setup = make_payment_setup(100_001);
    fund(&mut setup, 100_001);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_SPLIT, VOTE_SPLIT, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);

    // Post-fee principal is 90,001; the odd micro-Tari goes to the
    // payee, the payer keeps the floor half.
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_SPLIT);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 45_001);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 10_000);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 45_001
    );
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + 45_000
    );
    assert_eq!(vault_balance(&mut setup), 0);
}

// ----- matrix: fees capped at the principal -----

#[test]
fn settle_fees_capped_at_small_principal() {
    let mut setup = make_payment_setup(10_000);
    fund(&mut setup, 10_000);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);

    // The two majority jurors are owed 10,000 in fees, which is the
    // whole principal, so the payee and the payer both keep nothing.
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_PAY);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 0);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 10_000);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before);
    assert_eq!(vault_balance(&mut setup), 0);
    // The capped fees split 5,000 each, and the slashed 5,000 from the
    // minority tops them up by 2,500 each.
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[0]), 7_500);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[1]), 7_500);
    assert_eq!(dispute_claim(&mut setup, dispute_id, panel[2]), 0);
}

// ----- settlement guards -----

#[test]
fn settle_before_quorum_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // Only one juror reveals: no quorum, so no ruling exists to settle.
    let juror = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[0])
        .expect("drawn juror not found");
    let salt = hash_of(1);
    let commitment = vote_commitment(VOTE_PAY, salt, dispute_id, panel[0]);
    commit_vote(&mut setup, juror, dispute_id, commitment);
    reveal_vote(&mut setup, juror, dispute_id, VOTE_PAY, salt);

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
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "no majority ruling");
    assert_eq!(vault_balance(&mut setup), TOTAL);
}

#[test]
fn settle_without_majority_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    draw_panel(&mut setup, dispute_id);
    rule_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_PAY, VOTE_REFUND, VOTE_SPLIT],
    );

    // One vote each: no outcome holds a majority, so settle fails and
    // the money stays frozen in the escrow.
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
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "no majority ruling");
    assert_eq!(vault_balance(&mut setup), TOTAL);
}

#[test]
fn double_settlement_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);

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
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already settled");
    assert_eq!(vault_balance(&mut setup), 0);
}

// ----- fee claims -----

#[test]
fn claim_fee_pays_majority_and_rejects_double_claim() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);

    settle(&mut setup, &payer_secret, dispute_id);
    let majority = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[0])
        .expect("drawn juror not found");
    let minority = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[2])
        .expect("drawn juror not found");
    let majority_before = balance(&mut setup.test, majority);
    claim_fee(&mut setup, majority, dispute_id);
    assert_eq!(balance(&mut setup.test, majority), majority_before + 7_500);
    let claimed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_claimed",
        args![dispute_id, panel[0]],
        vec![],
    );
    assert!(claimed);

    // A second claim by the same juror fails, and the minority juror,
    // who is owed nothing, cannot claim at all.
    let verdict = setup.verdict;
    let majority_secret = majority.secret.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "claim_fee", args![dispute_id, majority.account])
            .build_and_seal(&majority_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already claimed");
    let minority_secret = minority.secret.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "claim_fee", args![dispute_id, minority.account])
            .build_and_seal(&minority_secret),
        vec![],
    );
    assert_reject_reason(&reason, "no fee claim");
}

// ----- replacement draw -----

#[test]
fn replacement_draw_recovers_after_no_show() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // Two jurors reveal, one each way: no majority, and the third juror
    // never reveals. The round has failed.
    rule_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &panel,
        &[VOTE_PAY, VOTE_REFUND],
    );
    let failed_key = panel[2];

    // Record the failure: the replacement commits a new future epoch.
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let replacement_used: bool = setup.test.call_method(
        setup.verdict,
        "dispute_replacement_used",
        args![dispute_id],
        vec![],
    );
    assert!(replacement_used);
    let draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    assert_eq!(draw_epoch, DRAW_DELAY + DRAW_DELAY);

    // Draw again in the new epoch, rule with a pay majority, settle.
    set_epoch(&mut setup.test, DRAW_DELAY + DRAW_DELAY);
    set_epoch_hash(&mut setup.test, hash_of(0xE4));
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    rule_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    );
    settle(&mut setup, &payer_secret, dispute_id);

    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_PAY);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 490_000
    );
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before);
    assert_eq!(vault_balance(&mut setup), 0);
    // The juror who failed to reveal in the first round is slashed
    // once, even though the replacement drew them again.
    assert_eq!(juror_bond(&mut setup, failed_key), BOND_FLOOR - JUROR_FEE);
    // Slash pool: 5,000 from the failed juror plus 5,000 from the
    // second-round minority, split between the two majority jurors.
    let new_panel = panel_keys(&mut setup, dispute_id);
    assert_eq!(dispute_claim(&mut setup, dispute_id, new_panel[0]), 10_000);
    assert_eq!(dispute_claim(&mut setup, dispute_id, new_panel[1]), 10_000);
    assert_eq!(dispute_claim(&mut setup, dispute_id, new_panel[2]), 0);
}

/// Commit and reveal votes for the first `votes.len()` panel jurors
/// only, leaving the rest of the panel unrevealed.
fn rule_panel_partial(
    setup: &mut Setup,
    jurors: &[Party],
    dispute_id: u64,
    panel: &[RistrettoPublicKeyBytes],
    votes: &[u64],
) {
    for (i, vote) in votes.iter().enumerate() {
        let key = panel[i];
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == key)
            .expect("drawn juror not found");
        let salt = hash_of((i + 1) as u8);
        let commitment = vote_commitment(*vote, salt, dispute_id, key);
        commit_vote(setup, juror, dispute_id, commitment);
        reveal_vote(setup, juror, dispute_id, *vote, salt);
    }
}

// ----- no-ruling fallback -----

#[test]
fn no_ruling_fallback_returns_principal() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // One juror reveals pay, the other two never reveal: no quorum.
    rule_panel_partial(&mut setup, &jurors, dispute_id, &panel, &[VOTE_PAY]);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    // Second round: one juror reveals refund, the rest stay silent.
    set_epoch(&mut setup.test, DRAW_DELAY + DRAW_DELAY);
    set_epoch_hash(&mut setup.test, hash_of(0xE4));
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let new_panel = panel_keys(&mut setup, dispute_id);
    rule_panel_partial(&mut setup, &jurors, dispute_id, &new_panel, &[VOTE_REFUND]);

    // The replacement is used and still no majority: the dispute closes
    // with no ruling, and the whole principal returns to the payer.
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), NO_OUTCOME);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before + TOTAL);
    assert_eq!(vault_balance(&mut setup), 0);
    let terminal: bool = setup
        .test
        .call_method(setup.escrow, "is_terminal", args![], vec![]);
    assert!(terminal);
    // Settle after a no-ruling close fails: the ruling can never run.
    let payee_account = setup.payee.account;
    let payer_account = setup.payer.account;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "settle",
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "closed with no ruling");
}

#[test]
fn refuse_votes_block_settlement_and_close() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    // Two jurors refuse to arbitrate, one rules pay: no outcome holds
    // a majority, so settle fails, but the round is fully revealed, so
    // the dispute can close with no ruling.
    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]);
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
                args![dispute_id, payee_account, payer_account],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "no majority ruling");

    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before + TOTAL);
    assert_eq!(vault_balance(&mut setup), 0);
    // Refusing jurors are neither paid nor slashed: their bonds are
    // untouched, and nobody is owed a fee.
    for key in &panel {
        assert_eq!(juror_bond(&mut setup, *key), BOND_FLOOR);
        assert_eq!(dispute_claim(&mut setup, dispute_id, *key), 0);
    }
}

// ----- reschedule -----

#[test]
fn reschedule_draw_after_missed_window() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let _jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    assert_eq!(draw_epoch, DRAW_DELAY);

    // The locked epoch passes without a draw: reschedule to the next.
    set_epoch(&mut setup.test, DRAW_DELAY + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "reschedule_draw", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    assert_eq!(draw_epoch, DRAW_DELAY + 1);
    let reschedules: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_reschedules",
        args![dispute_id],
        vec![],
    );
    assert_eq!(reschedules, 1);

    // The draw now executes in the rescheduled epoch.
    set_epoch_hash(&mut setup.test, hash_of(0xE2));
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let drawn: bool =
        setup
            .test
            .call_method(setup.verdict, "dispute_drawn", args![dispute_id], vec![]);
    assert!(drawn);
}

// ----- appeal -----

fn open_appeal(setup: &mut Setup, appellant: &Party, dispute_id: u64, fee: u64) {
    let verdict = setup.verdict;
    let appellant_account = appellant.account;
    let appellant_secret = appellant.secret.clone();
    let appellant_proof = appellant.proof.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(appellant_account, "withdraw", args![TARI_TOKEN, fee])
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                verdict,
                "open_appeal",
                args![
                    dispute_id,
                    DRAW_DELAY,
                    Workspace("appeal_fee"),
                    appellant_account
                ],
            )
            .build_and_seal(&appellant_secret),
        vec![appellant_proof],
    );
}

fn draw_appeal_panel(setup: &mut Setup, dispute_id: u64) -> Vec<RistrettoPublicKeyBytes> {
    let payer_secret = setup.payer.secret.clone();
    let draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_appeal_draw_epoch",
        args![dispute_id],
        vec![],
    );
    set_epoch(&mut setup.test, draw_epoch);
    set_epoch_hash(&mut setup.test, hash_of(0xA7));
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(setup.verdict, "draw_appeal_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    appeal_panel_keys(setup, dispute_id)
}

fn commit_appeal_vote(setup: &mut Setup, juror: &Party, dispute_id: u64, commitment: Hash32) {
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_appeal_vote", args![dispute_id, commitment])
            .build_and_seal(&juror_secret),
        vec![],
    );
}

fn reveal_appeal_vote(setup: &mut Setup, juror: &Party, dispute_id: u64, vote: u64, salt: Hash32) {
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "reveal_appeal_vote", args![dispute_id, vote, salt])
            .build_and_seal(&juror_secret),
        vec![],
    );
}

/// Rule the appeal with the given votes, one per appeal panel juror in
/// appeal panel order.
fn rule_appeal_panel(setup: &mut Setup, jurors: &[Party], dispute_id: u64, votes: &[u64]) {
    let panel = appeal_panel_keys(setup, dispute_id);
    assert_eq!(panel.len(), votes.len(), "votes must cover the panel");
    for (i, key) in panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn appeal juror not found");
        let salt = hash_of((i + 11) as u8);
        let commitment = vote_commitment(votes[i], salt, dispute_id, *key);
        commit_appeal_vote(setup, juror, dispute_id, commitment);
        reveal_appeal_vote(setup, juror, dispute_id, votes[i], salt);
    }
}

#[test]
fn appeal_overturns_first_ruling() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    // First round rules pay, two to one.
    let (dispute_id, first_panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    // The payer appeals, posting the full appeal fee.
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let appealed: bool =
        setup
            .test
            .call_method(setup.verdict, "dispute_appealed", args![dispute_id], vec![]);
    assert!(appealed);
    let appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    assert_eq!(appeal_panel.len(), 7);

    // The appeal rules refund, four to three. The first-round pay
    // majority keeps its fee claim; the appeal majority is paid from
    // the appeal fee.
    let mut appeal_votes = [VOTE_PAY; 7];
    let mut refunds = 0;
    for (i, key) in appeal_panel.iter().enumerate() {
        if refunds < 4 && !first_panel[0..2].contains(key) {
            appeal_votes[i] = VOTE_REFUND;
            refunds += 1;
        }
    }
    assert_eq!(refunds, 4, "appeal must rule refund four to three");
    rule_appeal_panel(&mut setup, &jurors, dispute_id, &appeal_votes);
    settle(&mut setup, &payer_secret, dispute_id);

    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_REFUND);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 0);
    // The payer posted 35,000 appeal fee, gets back 490,000 of
    // principal and the 15,000 unspent appeal fee. The 500,000 funding
    // is already out of the balance measured before the dispute.
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before - APPEAL_FEE + 490_000 + 15_000
    );
    assert_eq!(vault_balance(&mut setup), 0);
    // First-round majority jurors: one fee plus a 2,500 slash share
    // each, from the one first-round minority juror.
    for key in &first_panel[0..2] {
        assert_eq!(dispute_claim(&mut setup, dispute_id, *key), 7_500);
    }
    // The first-round minority juror is owed nothing for round one.
    // In the appeal they are owed a fee only if they voted with the
    // appeal majority.
    let appeal_index = appeal_panel
        .iter()
        .position(|key| key == &first_panel[2])
        .expect("first-round minority juror not in appeal panel");
    let expected_minority_claim = if appeal_votes[appeal_index] == VOTE_REFUND {
        8_750
    } else {
        0
    };
    assert_eq!(
        dispute_claim(&mut setup, dispute_id, first_panel[2]),
        expected_minority_claim
    );
    // Appeal majority jurors: one fee plus a 3,750 slash share each,
    // from the three appeal minority jurors (15,000 split four ways).
    for (i, key) in appeal_panel.iter().enumerate() {
        if appeal_votes[i] == VOTE_REFUND {
            assert_eq!(dispute_claim(&mut setup, dispute_id, *key), 8_750);
        } else {
            // Appeal minority jurors who were not in the first-round
            // majority are owed nothing.
            let first_claim = dispute_claim(&mut setup, dispute_id, *key);
            assert!(
                first_claim == 0 || first_claim == 7_500,
                "unexpected claim {first_claim} for appeal minority juror"
            );
        }
    }
}

#[test]
fn appeal_guards_reject_bad_fee_stranger_and_double_appeal() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);

    // A stranger cannot appeal: only a party to the dispute can.
    let verdict = setup.verdict;
    let stranger = setup.stranger.clone();
    let stranger_secret = stranger.secret.clone();
    let stranger_proof = stranger.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(stranger.account, "withdraw", args![TARI_TOKEN, APPEAL_FEE])
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                verdict,
                "open_appeal",
                args![
                    dispute_id,
                    DRAW_DELAY,
                    Workspace("appeal_fee"),
                    stranger.account
                ],
            )
            .build_and_seal(&stranger_secret),
        vec![stranger_proof],
    );
    assert_reject_reason(&reason, "only a party");

    // The wrong fee is rejected: the appeal fee is exactly APPEAL_FEE.
    let payer = setup.payer.clone();
    let payer_proof = payer.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer.account, "withdraw", args![TARI_TOKEN, APPEAL_FEE - 1])
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                verdict,
                "open_appeal",
                args![
                    dispute_id,
                    DRAW_DELAY,
                    Workspace("appeal_fee"),
                    payer.account
                ],
            )
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "exactly APPEAL_FEE");

    // A valid appeal opens, and a second appeal is rejected.
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let payer_proof = setup.payer.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                setup.payer.account,
                "withdraw",
                args![TARI_TOKEN, APPEAL_FEE],
            )
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                verdict,
                "open_appeal",
                args![
                    dispute_id,
                    DRAW_DELAY,
                    Workspace("appeal_fee"),
                    setup.payer.account
                ],
            )
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "already opened");
}

#[test]
fn appeal_after_settlement_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);

    // Once settled, the money has moved: there is nothing to appeal.
    let payer = setup.payer.clone();
    let payer_proof = payer.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer.account, "withdraw", args![TARI_TOKEN, APPEAL_FEE])
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                setup.verdict,
                "open_appeal",
                args![
                    dispute_id,
                    DRAW_DELAY,
                    Workspace("appeal_fee"),
                    payer.account
                ],
            )
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "already settled");
}
