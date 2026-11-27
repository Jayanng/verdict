// Engine tests: the settlement matrix, juror fees and
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
const REVEAL_WINDOW: u64 = 2;
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
    let mut revealed: Vec<(&Party, u64, Hash32)> = Vec::new();
    for (i, key) in panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn juror not found");
        let salt = hash_of((i + 1) as u8);
        let commitment = vote_commitment(votes[i], salt, dispute_id, *key);
        commit_vote(setup, juror, dispute_id, commitment);
        revealed.push((juror, votes[i], salt));
    }
    // The commit window ends at draw + 1, so reveals happen at the
    // reveal deadline (draw + 2): after the commit window ends, before
    // it closes.
    let deadline = reveal_deadline(setup, dispute_id);
    set_epoch(&mut setup.test, deadline);
    for (juror, vote, salt) in revealed {
        reveal_vote(setup, juror, dispute_id, vote, salt);
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

/// Settle a dispute. Settle runs only once the reveal window has
/// passed, so the epoch moves to the latest reveal deadline first:
/// the first-round deadline, and the appeal deadline when the dispute
/// is under appeal.
fn settle(setup: &mut Setup, signer: &RistrettoSecretKey, dispute_id: u64) {
    let mut deadline = reveal_deadline(setup, dispute_id);
    let appealed: bool =
        setup
            .test
            .call_method(setup.verdict, "dispute_appealed", args![dispute_id], vec![]);
    if appealed {
        deadline = deadline.max(appeal_reveal_deadline(setup, dispute_id));
    }
    // Settle requires an epoch strictly after the reveal deadline, so a
    // juror whose reveal is still valid in the deadline epoch cannot be
    // slashed by a settlement in that epoch.
    set_epoch(&mut setup.test, deadline + 1);
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

fn dispute_payer_amount(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_payer_amount",
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
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
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
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), 0);
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
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), 490_000);
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
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), 245_000);
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
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), 45_000);
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
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), 0);
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

// ----- degenerate split on appeal keeps the adjudicated outcome -----

#[test]
fn settle_appeal_split_at_small_principal_keeps_split_jurors() {
    let mut setup = make_payment_setup(10_000);
    fund(&mut setup, 10_000);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    // First round rules pay two to one, so the fees are 10,000, the
    // whole principal: a split here would route (0, 10,000).
    let (dispute_id, first_panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    // The appeal majority votes split, four to three. The first-round
    // refund voter gets a refund vote in the appeal too, so they are
    // an offender in both rounds; every other appeal juror's vote is
    // assigned by panel order.
    let refund_key = first_panel[2];
    let mut appeal_votes = [VOTE_PAY; 7];
    let mut split_assigned = 0;
    let mut pay_assigned = 0;
    for (i, key) in appeal_panel.iter().enumerate() {
        if *key == refund_key {
            appeal_votes[i] = VOTE_REFUND;
        } else if split_assigned < 4 {
            appeal_votes[i] = VOTE_SPLIT;
            split_assigned += 1;
        } else if pay_assigned < 2 {
            appeal_votes[i] = VOTE_PAY;
            pay_assigned += 1;
        } else {
            appeal_votes[i] = VOTE_REFUND;
        }
    }
    // The shape is four split, two pay, one refund in every case:
    // the refund voter takes one slot, and the other six slots fill
    // with four split and then two pay.
    rule_appeal_panel(&mut setup, &jurors, dispute_id, &appeal_votes);
    settle(&mut setup, &payer_secret, dispute_id);
    // The payout is the pay conversion, (0, 10,000), because the
    // post-fee principal is empty. But the recorded outcome is the
    // one the appeal jury actually reached: split, not pay.
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_SPLIT);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 0);
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), 0);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 10_000);
    assert_eq!(vault_balance(&mut setup), 0);
    // The four split jurors are the appeal majority: they are not
    // offenders, so their bonds are untouched, and each is owed a
    // 5,000 fee plus a 3,750 share of the 15,000 slashed from the
    // three appeal offenders, 8,750 in total. A split juror who was
    // also a first-round majority juror adds the 7,500 first-round
    // claim on top.
    for (i, key) in appeal_panel.iter().enumerate() {
        let vote = appeal_votes[i];
        let first_majority = first_panel[0..2].contains(key);
        if vote == VOTE_SPLIT {
            assert_eq!(juror_bond(&mut setup, *key), BOND_FLOOR);
            let expected = 8_750 + if first_majority { 7_500 } else { 0 };
            assert_eq!(dispute_claim(&mut setup, dispute_id, *key), expected);
        } else if vote == VOTE_PAY {
            assert_eq!(juror_bond(&mut setup, *key), BOND_FLOOR - JUROR_FEE);
            let expected = if first_majority { 7_500 } else { 0 };
            assert_eq!(dispute_claim(&mut setup, dispute_id, *key), expected);
        } else {
            // Refund voter: an offender in the appeal. The first-round
            // refund voter is also an offender in the first round, so
            // they are slashed twice.
            let expected_bond = if *key == refund_key {
                BOND_FLOOR - 2 * JUROR_FEE
            } else {
                BOND_FLOOR - JUROR_FEE
            };
            assert_eq!(juror_bond(&mut setup, *key), expected_bond);
            let expected = if first_majority { 7_500 } else { 0 };
            assert_eq!(dispute_claim(&mut setup, dispute_id, *key), expected);
        }
    }
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
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline);
    reveal_vote(&mut setup, juror, dispute_id, VOTE_PAY, salt);

    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
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
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
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
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
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

    // The reveal window must pass first: the replacement commits a new
    // future epoch, and it cannot land while honest jurors are still
    // within their time to reveal. The draw was at epoch 2, so the
    // deadline is epoch 4, the replacement runs at epoch 5, and the
    // replacement draw locks to epoch 7.
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1);
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
    assert_eq!(draw_epoch, DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY);

    // Draw again in the new epoch, rule with a pay majority, settle.
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY);
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
    // Slash pool: one juror fee per distinct offender, the failed juror
    // plus the second-round minority, split between the two majority
    // jurors. Whether the minority voter is the failed juror depends on
    // the panel the draw produces, so the expected claim is computed
    // from the distinct offenders, not hardcoded.
    let new_panel = panel_keys(&mut setup, dispute_id);
    let distinct_offenders = if new_panel[2] == failed_key { 1 } else { 2 };
    let expected_claim = JUROR_FEE + JUROR_FEE * distinct_offenders / 2;
    assert_eq!(
        dispute_claim(&mut setup, dispute_id, new_panel[0]),
        expected_claim
    );
    assert_eq!(
        dispute_claim(&mut setup, dispute_id, new_panel[1]),
        expected_claim
    );
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
    let mut revealed: Vec<(&Party, u64, Hash32)> = Vec::new();
    for (i, vote) in votes.iter().enumerate() {
        let key = panel[i];
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == key)
            .expect("drawn juror not found");
        let salt = hash_of((i + 1) as u8);
        let commitment = vote_commitment(*vote, salt, dispute_id, key);
        commit_vote(setup, juror, dispute_id, commitment);
        revealed.push((juror, *vote, salt));
    }
    // The commit window ends at draw + 1, so reveals happen at the
    // reveal deadline (draw + 2): after the commit window ends, before
    // it closes.
    let deadline = reveal_deadline(setup, dispute_id);
    set_epoch(&mut setup.test, deadline);
    for (juror, vote, salt) in revealed {
        reveal_vote(setup, juror, dispute_id, vote, salt);
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
    // The reveal window must pass before the replacement can run.
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1);
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
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY);
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

    // The replacement round has had its full window: the new draw was
    // at epoch 7, so its reveal deadline is epoch 9, and only then is
    // the round finished with no majority, so the dispute closes with
    // no ruling and the whole principal returns to the payer.
    set_epoch(
        &mut setup.test,
        DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY + REVEAL_WINDOW + 1,
    );
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), NO_OUTCOME);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 0);
    assert_eq!(dispute_payer_amount(&mut setup, dispute_id), TOTAL);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before + TOTAL);
    assert_eq!(vault_balance(&mut setup), 0);
    let terminal: bool = setup
        .test
        .call_method(setup.escrow, "is_terminal", args![], vec![]);
    assert!(terminal);
    // Settle after a no-ruling close fails: the ruling can never run.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
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
    // the dispute can close with no ruling, once the reveal window
    // has passed: the close runs strictly after the deadline, never
    // in the deadline epoch.
    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "no majority ruling");

    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
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

    // The locked epoch passes without a draw: reschedule past the
    // current epoch, never onto it. The draw was locked to epoch 2, the
    // current epoch is 3, so the new draw epoch is 4.
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
    assert_eq!(draw_epoch, DRAW_DELAY + 2);
    let reschedules: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_reschedules",
        args![dispute_id],
        vec![],
    );
    assert_eq!(reschedules, 1);

    // The draw now executes in the rescheduled epoch.
    set_epoch(&mut setup.test, DRAW_DELAY + 2);
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
    open_appeal_with_delay(setup, appellant, dispute_id, fee, DRAW_DELAY);
}

fn open_appeal_with_delay(
    setup: &mut Setup,
    appellant: &Party,
    dispute_id: u64,
    fee: u64,
    delay: u64,
) {
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
                args![dispute_id, delay, Workspace("appeal_fee")],
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
    let mut revealed: Vec<(&Party, u64, Hash32)> = Vec::new();
    for (i, key) in panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn appeal juror not found");
        let salt = hash_of((i + 11) as u8);
        let commitment = vote_commitment(votes[i], salt, dispute_id, *key);
        commit_appeal_vote(setup, juror, dispute_id, commitment);
        revealed.push((juror, votes[i], salt));
    }
    let deadline = appeal_reveal_deadline(setup, dispute_id);
    set_epoch(&mut setup.test, deadline);
    for (juror, vote, salt) in revealed {
        reveal_appeal_vote(setup, juror, dispute_id, vote, salt);
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
    // Seven jurors, so the appeal can draw a full panel of seven.
    let jurors = register_jurors(&mut setup, 7);
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
                args![dispute_id, DRAW_DELAY, Workspace("appeal_fee")],
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
                args![dispute_id, DRAW_DELAY, Workspace("appeal_fee")],
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
                args![dispute_id, DRAW_DELAY, Workspace("appeal_fee")],
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
                args![dispute_id, DRAW_DELAY, Workspace("appeal_fee")],
            )
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "already settled");
}

// ----- draw delay, reveal window, pool size, draw domain -----

/// Rule the appeal with the given votes for the first `votes.len()`
/// appeal panel jurors only, leaving the rest unrevealed.
fn rule_appeal_panel_partial(setup: &mut Setup, jurors: &[Party], dispute_id: u64, votes: &[u64]) {
    let panel = appeal_panel_keys(setup, dispute_id);
    let mut revealed: Vec<(&Party, u64, Hash32)> = Vec::new();
    for (i, vote) in votes.iter().enumerate() {
        let key = panel[i];
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == key)
            .expect("drawn appeal juror not found");
        let salt = hash_of((i + 11) as u8);
        let commitment = vote_commitment(*vote, salt, dispute_id, key);
        commit_appeal_vote(setup, juror, dispute_id, commitment);
        revealed.push((juror, *vote, salt));
    }
    let deadline = appeal_reveal_deadline(setup, dispute_id);
    set_epoch(&mut setup.test, deadline);
    for (juror, vote, salt) in revealed {
        reveal_appeal_vote(setup, juror, dispute_id, vote, salt);
    }
}

/// Open a dispute, rule the first round pay two to one, open an appeal,
/// and draw the appeal panel. Returns the dispute id, the first panel,
/// and the appeal panel.
fn dispute_with_appeal_drawn(
    setup: &mut Setup,
    jurors: &[Party],
) -> (
    u64,
    Vec<RistrettoPublicKeyBytes>,
    Vec<RistrettoPublicKeyBytes>,
) {
    let (dispute_id, first_panel) =
        dispute_with_ruling(setup, jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let payer = setup.payer.clone();
    open_appeal(setup, &payer, dispute_id, APPEAL_FEE);
    let appeal_panel = draw_appeal_panel(setup, dispute_id);
    (dispute_id, first_panel, appeal_panel)
}

fn appeal_draw_epoch(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_appeal_draw_epoch",
        args![dispute_id],
        vec![],
    )
}

fn reveal_deadline(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_reveal_deadline",
        args![dispute_id],
        vec![],
    )
}

fn commit_deadline(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_commit_deadline",
        args![dispute_id],
        vec![],
    )
}

fn appeal_reveal_deadline(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_appeal_reveal_deadline",
        args![dispute_id],
        vec![],
    )
}

fn appeal_commit_deadline(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_appeal_commit_deadline",
        args![dispute_id],
        vec![],
    )
}

#[test]
fn open_dispute_no_delay_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let _jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "open_dispute", args![setup.escrow, 0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "draw delay must be at least one epoch");
}

#[test]
fn replace_jury_no_delay_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    rule_panel_partial(&mut setup, &jurors, dispute_id, &panel, &[VOTE_PAY]);
    // The reveal window has passed, but the delay itself is still 0.
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, 0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "draw delay must be at least one epoch");
}

#[test]
fn open_appeal_no_delay_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let verdict = setup.verdict;
    let payer = setup.payer.clone();
    let payer_proof = payer.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer.account, "withdraw", args![TARI_TOKEN, APPEAL_FEE])
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                verdict,
                "open_appeal",
                args![dispute_id, 0u64, Workspace("appeal_fee")],
            )
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "draw delay must be at least one epoch");
}

#[test]
fn replace_appeal_jury_no_delay_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    // Three refund, three pay, one silent: no majority, and the appeal
    // reveal window has passed.
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
        ],
    );
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_appeal_jury", args![dispute_id, 0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "draw delay must be at least one epoch");
}

#[test]
fn replace_jury_rejected_before_reveal_deadline() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    rule_panel_partial(&mut setup, &jurors, dispute_id, &panel, &[VOTE_PAY]);
    // Still inside the reveal window: the draw was at epoch 2 and the
    // deadline is epoch 4, so a replacement now would catch jurors who
    // still have time to reveal.
    let deadline = reveal_deadline(&mut setup, dispute_id);
    assert_eq!(deadline, DRAW_DELAY + REVEAL_WINDOW);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "reveal window has not passed yet");
}

#[test]
fn replace_jury_allowed_for_stranger() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    rule_panel_partial(&mut setup, &jurors, dispute_id, &panel, &[VOTE_PAY]);
    // The window has passed, and replacement is permissionless: any
    // caller can advance an incomplete round, so the dispute does not
    // depend on a party submitting a transaction.
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    let stranger = setup.stranger.clone();
    let stranger_secret = stranger.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&stranger_secret),
        vec![],
    );
    let replaced: bool = setup.test.call_method(
        setup.verdict,
        "dispute_replacement_used",
        args![dispute_id],
        vec![],
    );
    assert!(replaced);
}

#[test]
fn replace_appeal_jury_rejected_before_reveal_deadline() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_REFUND, VOTE_REFUND, VOTE_REFUND],
    );
    // Still inside the appeal reveal window.
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    assert_eq!(
        deadline,
        appeal_draw_epoch(&mut setup, dispute_id) + REVEAL_WINDOW
    );
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "replace_appeal_jury",
                args![dispute_id, DRAW_DELAY],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal reveal window has not passed yet");
}

#[test]
fn replace_appeal_jury_allowed_for_stranger() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_REFUND, VOTE_REFUND, VOTE_REFUND],
    );
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    let stranger = setup.stranger.clone();
    let stranger_secret = stranger.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "replace_appeal_jury",
                args![dispute_id, DRAW_DELAY],
            )
            .build_and_seal(&stranger_secret),
        vec![],
    );
    let replaced: bool = setup.test.call_method(
        setup.verdict,
        "dispute_appeal_replacement_used",
        args![dispute_id],
        vec![],
    );
    assert!(replaced);
}

#[test]
fn appeal_replacement_recovers_after_no_show() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    // Three refund, three pay, one silent: no majority, and the appeal
    // round has failed.
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
        ],
    );
    // The appeal reveal window must pass first, then a party replaces.
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "replace_appeal_jury",
                args![dispute_id, DRAW_DELAY],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    let used: bool = setup.test.call_method(
        setup.verdict,
        "dispute_appeal_replacement_used",
        args![dispute_id],
        vec![],
    );
    assert!(used);
    assert_eq!(
        appeal_draw_epoch(&mut setup, dispute_id),
        deadline + 1 + DRAW_DELAY
    );
    // Draw the replacement panel in the new epoch, rule refund four to
    // three, settle.
    let appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    assert_eq!(appeal_panel.len(), 7);
    let mut appeal_votes = [VOTE_PAY; 7];
    for vote in appeal_votes.iter_mut().take(4) {
        *vote = VOTE_REFUND;
    }
    rule_appeal_panel(&mut setup, &jurors, dispute_id, &appeal_votes);
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_REFUND);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 0);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before - APPEAL_FEE + 490_000 + 15_000
    );
    assert_eq!(vault_balance(&mut setup), 0);
}

#[test]
fn open_dispute_pool_too_small_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    // Two jurors cannot fill a panel of three.
    let _jurors = register_jurors(&mut setup, 2);
    let payer_secret = setup.payer.secret.clone();
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "open_dispute", args![setup.escrow, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "not enough eligible jurors");
}

#[test]
fn open_appeal_pool_too_small_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    // Three jurors fill a first-round panel but not an appeal panel
    // of seven.
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let verdict = setup.verdict;
    let payer = setup.payer.clone();
    let payer_proof = payer.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(payer.account, "withdraw", args![TARI_TOKEN, APPEAL_FEE])
            .put_last_instruction_output_on_workspace("appeal_fee")
            .call_method(
                verdict,
                "open_appeal",
                args![dispute_id, DRAW_DELAY, Workspace("appeal_fee")],
            )
            .build_and_seal(&payer_secret),
        vec![payer_proof],
    );
    assert_reject_reason(&reason, "not enough eligible jurors");
}

#[test]
fn appeal_panel_differs_from_first_round_with_same_hash() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, first_panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    // The appeal draws from the same epoch hash as the first round.
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    set_epoch(&mut setup.test, draw_epoch);
    set_epoch_hash(&mut setup.test, hash_of(0xE2));
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(setup.verdict, "draw_appeal_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let appeal_panel = appeal_panel_keys(&mut setup, dispute_id);
    assert_eq!(appeal_panel.len(), 7);
    // Same hash, same pool, same dispute, round 0 in both: only the
    // draw domain differs, so the panels must differ.
    let mut first_sorted = first_panel.clone();
    let mut appeal_sorted = appeal_panel.clone();
    first_sorted.sort();
    appeal_sorted.sort();
    assert_ne!(
        first_sorted,
        appeal_sorted[0..3].to_vec(),
        "appeal panel must differ from the first-round panel"
    );
    assert_ne!(
        first_sorted, appeal_sorted,
        "appeal panel must differ from the first-round panel"
    );
}

#[test]
fn reschedule_appeal_draw_moves_beyond_current_epoch() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    // The appeal draw epoch passes without a draw: reschedule past the
    // current epoch, never onto it.
    set_epoch(&mut setup.test, draw_epoch + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "reschedule_appeal_draw", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let new_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    assert_eq!(new_epoch, draw_epoch + 2);
    let reschedules: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_appeal_reschedules",
        args![dispute_id],
        vec![],
    );
    assert_eq!(reschedules, 1);
    // The appeal draw now executes in the rescheduled epoch.
    let appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    assert_eq!(appeal_panel.len(), 7);
}

// ----- account derivation and caller-supplied accounts -----

#[test]
fn account_address_for_key_matches_party_accounts() {
    let mut setup = make_setup();
    let payer_key = setup.payer.pk_bytes();
    let payee_key = setup.payee.pk_bytes();
    let payer_account: ComponentAddress = setup.test.call_method(
        setup.verdict,
        "account_address_for_key",
        args![payer_key],
        vec![],
    );
    let payee_account: ComponentAddress = setup.test.call_method(
        setup.verdict,
        "account_address_for_key",
        args![payee_key],
        vec![],
    );
    assert_eq!(payer_account, setup.payer.account);
    assert_eq!(payee_account, setup.payee.account);
}

#[test]
fn settle_by_stranger_pays_escrow_parties_only() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);
    let stranger_before = balance(&mut setup.test, &setup.stranger);
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    // Settle takes no accounts from the caller, so a stranger can trigger
    // it but cannot redirect the proceeds: the money still lands with
    // the escrow payer and payee, and the stranger gains nothing.
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    let stranger_secret = setup.stranger.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
            .build_and_seal(&stranger_secret),
        vec![],
    );
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_PAY);
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 490_000
    );
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before);
    assert_eq!(balance(&mut setup.test, &setup.stranger), stranger_before);
    assert_eq!(vault_balance(&mut setup), 0);
}

// ----- reveal deadline check on settle -----

#[test]
fn settle_before_reveal_deadline_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    // Two of three have revealed and a majority exists, but the third
    // juror is still inside the reveal window: settle must wait, so
    // the late juror is never slashed for a window still open.
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "reveal window has not passed yet");
    assert_eq!(vault_balance(&mut setup), TOTAL);
    // At the deadline the same settle succeeds, and the juror who never
    // revealed is slashed then, with the window closed.
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_PAY);
}

// ----- close after a replacement -----

#[test]
fn close_after_replacement_rejected_until_replacement_round_finishes() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // One juror reveals pay, the rest stay silent: no majority.
    rule_panel_partial(&mut setup, &jurors, dispute_id, &panel, &[VOTE_PAY]);
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    // The replacement has just run and no new round has been drawn
    // yet: the close must not run before the new jurors have had time
    // to vote, and the first round can still reach a ruling.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "first round can still reach a ruling");
    // Draw the replacement round and reveal one vote: still not
    // finished, since the new window has not passed and the round is
    // not fully revealed.
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY);
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
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "first round can still reach a ruling");
    // Once the replacement round's own window has passed with no
    // majority, the round is finished and the close runs.
    set_epoch(
        &mut setup.test,
        DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY + REVEAL_WINDOW + 1,
    );
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
}

// ----- refusing juror in the no-ruling close -----

#[test]
fn close_refusing_juror_is_neither_paid_nor_slashed() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // First round: one refuse, one pay, one silent. No majority.
    rule_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &panel,
        &[VOTE_REFUSE, VOTE_PAY],
    );
    let failed_first = panel[2];
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    // Replacement round: one refuse, one pay, one silent again.
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY);
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
    rule_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &new_panel,
        &[VOTE_REFUSE, VOTE_PAY],
    );
    set_epoch(
        &mut setup.test,
        DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY + REVEAL_WINDOW + 1,
    );
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    // The refusing juror in the closing round is owed nothing: the
    // slashed bonds go to the paying juror alone.
    let refuse_key = new_panel[0];
    let pay_key = new_panel[1];
    let silent_key = new_panel[2];
    assert_eq!(dispute_claim(&mut setup, dispute_id, refuse_key), 0);
    // Slashed bonds: the silent juror of the closing round plus the
    // juror who failed in the first round, one fee each, unless they
    // are the same juror.
    let expected_pay_claim = if silent_key == failed_first {
        5_000
    } else {
        10_000
    };
    assert_eq!(
        dispute_claim(&mut setup, dispute_id, pay_key),
        expected_pay_claim
    );
    // The refusing juror is slashed only if they are also the juror who
    // failed to reveal in the first round; a refuse vote alone never
    // slashes.
    let expected_refuse_bond = if refuse_key == failed_first {
        BOND_FLOOR - JUROR_FEE
    } else {
        BOND_FLOOR
    };
    assert_eq!(juror_bond(&mut setup, refuse_key), expected_refuse_bond);
    assert_eq!(juror_bond(&mut setup, failed_first), BOND_FLOOR - JUROR_FEE);
    assert_eq!(juror_bond(&mut setup, silent_key), BOND_FLOOR - JUROR_FEE);
}

// ----- failed jurors stay in their own round -----

#[test]
fn appeal_failed_juror_bonds_top_up_appeal_majority_only() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, first_panel, appeal_panel) = dispute_with_appeal_drawn(&mut setup, &jurors);
    // Appeal round one: three refund, three pay, one silent. No majority.
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
        ],
    );
    let failed_appeal = appeal_panel[6];
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "replace_appeal_jury",
                args![dispute_id, DRAW_DELAY],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    // Appeal round two: refund four to three, and the dispute settles.
    let new_appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    let appeal_votes = [
        VOTE_REFUND,
        VOTE_REFUND,
        VOTE_REFUND,
        VOTE_REFUND,
        VOTE_PAY,
        VOTE_PAY,
        VOTE_PAY,
    ];
    rule_appeal_panel(&mut setup, &jurors, dispute_id, &appeal_votes);
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_REFUND);
    // First-round majority: one fee plus a share of the one
    // first-round minority slash, 7,500 each. No appeal slash leaks in.
    // Appeal majority: one fee plus a share of the appeal slash, one
    // fee per juror in the round-two minority plus the one failed
    // juror, counted once even when the failed juror also voted with
    // the minority in the second round.
    let mut appeal_slash_keys: Vec<RistrettoPublicKeyBytes> = new_appeal_panel
        .iter()
        .zip(appeal_votes.iter())
        .filter(|(_, vote)| **vote == VOTE_PAY)
        .map(|(key, _)| *key)
        .collect();
    if !appeal_slash_keys.contains(&failed_appeal) {
        appeal_slash_keys.push(failed_appeal);
    }
    let appeal_share_each = JUROR_FEE + (appeal_slash_keys.len() as u64 * JUROR_FEE) / 4;
    for juror in &jurors {
        let key = juror.pk_bytes();
        let first_share = if first_panel[0..2].contains(&key) {
            7_500
        } else {
            0
        };
        let appeal_index = new_appeal_panel
            .iter()
            .position(|k| k == &key)
            .expect("appeal juror not found");
        let appeal_share = if appeal_votes[appeal_index] == VOTE_REFUND {
            appeal_share_each
        } else {
            0
        };
        assert_eq!(
            dispute_claim(&mut setup, dispute_id, key),
            first_share + appeal_share,
            "claim mismatch for juror {key:?}"
        );
    }
    // The failed appeal juror is slashed once for the missed reveal.
    assert_eq!(
        juror_bond(&mut setup, failed_appeal),
        BOND_FLOOR - JUROR_FEE
    );
}

// ----- full-panel majority -----

#[test]
fn appeal_three_pay_of_four_revealed_is_no_ruling() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    // Four revealed, three pay and one refund: a majority of the
    // revealed votes, but not a strict majority of the full panel
    // of seven, so there is no ruling.
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    );
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "settle", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal round has no majority ruling");
}

#[test]
fn appeal_four_matching_pay_votes_rule_pay() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    // Four matching pay votes of seven is a strict majority of the
    // full panel, even with three jurors silent.
    rule_appeal_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_PAY],
    );
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_PAY);
    assert_eq!(dispute_payee_amount(&mut setup, dispute_id), 490_000);
    assert_eq!(vault_balance(&mut setup), 0);
}

// ----- reschedule cap -----

#[test]
fn reschedule_draw_capped_at_three() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let _jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let verdict = setup.verdict;
    let mut draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    for expected in 1..=3u64 {
        set_epoch(&mut setup.test, draw_epoch + 1);
        setup.test.execute_expect_success(
            setup
                .test
                .transaction()
                .call_method(verdict, "reschedule_draw", args![dispute_id])
                .build_and_seal(&payer_secret),
            vec![],
        );
        draw_epoch = setup.test.call_method(
            setup.verdict,
            "dispute_draw_epoch",
            args![dispute_id],
            vec![],
        );
        let reschedules: u64 = setup.test.call_method(
            setup.verdict,
            "dispute_reschedules",
            args![dispute_id],
            vec![],
        );
        assert_eq!(reschedules, expected);
    }
    // The fourth reschedule is refused: the draw cannot be put off
    // forever.
    set_epoch(&mut setup.test, draw_epoch + 1);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "reschedule_draw", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "reschedule limit reached");
}

#[test]
fn reschedule_appeal_draw_capped_at_three() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let verdict = setup.verdict;
    let mut draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    for expected in 1..=3u64 {
        set_epoch(&mut setup.test, draw_epoch + 1);
        setup.test.execute_expect_success(
            setup
                .test
                .transaction()
                .call_method(verdict, "reschedule_appeal_draw", args![dispute_id])
                .build_and_seal(&payer_secret),
            vec![],
        );
        draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
        let reschedules: u64 = setup.test.call_method(
            setup.verdict,
            "dispute_appeal_reschedules",
            args![dispute_id],
            vec![],
        );
        assert_eq!(reschedules, expected);
    }
    set_epoch(&mut setup.test, draw_epoch + 1);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "reschedule_appeal_draw", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "reschedule limit reached");
}

// ----- round 3: undrawn disputes, vote windows, appellant account -----

#[test]
fn undrawn_dispute_closes_after_reschedule_cap() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let verdict = setup.verdict;
    let payer_before = balance(&mut setup.test, &setup.payer);
    let payee_before = balance(&mut setup.test, &setup.payee);
    let mut draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    for _ in 0..2 {
        set_epoch(&mut setup.test, draw_epoch + 1);
        setup.test.execute_expect_success(
            setup
                .test
                .transaction()
                .call_method(verdict, "reschedule_draw", args![dispute_id])
                .build_and_seal(&payer_secret),
            vec![],
        );
        draw_epoch = setup.test.call_method(
            setup.verdict,
            "dispute_draw_epoch",
            args![dispute_id],
            vec![],
        );
    }
    // After two reschedules the close is still refused: the draw can
    // still move.
    set_epoch(&mut setup.test, draw_epoch + 1);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "first round can still reach a ruling");
    // The third reschedule spends the cap.
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "reschedule_draw", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    draw_epoch = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    // The final locked epoch has not passed yet, so the close is still
    // refused.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "first round can still reach a ruling");
    // Once the final epoch passes with no draw, the dispute closes
    // terminally: the whole principal returns to the payer, no juror
    // fee is taken, and no juror is slashed.
    set_epoch(&mut setup.test, draw_epoch + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before + TOTAL);
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(vault_balance(&mut setup), 0);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 0);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
    for juror in &jurors {
        assert_eq!(juror_bond(&mut setup, juror.pk_bytes()), BOND_FLOOR);
    }
}

#[test]
fn undrawn_appeal_closes_after_reschedule_cap() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let verdict = setup.verdict;
    let payer_before = balance(&mut setup.test, &setup.payer);
    let payee_before = balance(&mut setup.test, &setup.payee);
    let mut draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    for _ in 0..2 {
        set_epoch(&mut setup.test, draw_epoch + 1);
        setup.test.execute_expect_success(
            setup
                .test
                .transaction()
                .call_method(verdict, "reschedule_appeal_draw", args![dispute_id])
                .build_and_seal(&payer_secret),
            vec![],
        );
        draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    }
    // After two reschedules the close is still refused.
    set_epoch(&mut setup.test, draw_epoch + 1);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal round can still reach a ruling");
    // The third reschedule spends the cap.
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "reschedule_appeal_draw", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    draw_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    // The final locked epoch has not passed yet.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal round can still reach a ruling");
    // Once the final epoch passes, the dispute closes: the principal
    // returns to the payer, the appeal fee returns in full, and no
    // juror fee is taken.
    set_epoch(&mut setup.test, draw_epoch + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + TOTAL + APPEAL_FEE
    );
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(vault_balance(&mut setup), 0);
    assert_eq!(dispute_fees_paid(&mut setup, dispute_id), 0);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
}

#[test]
fn commit_after_commit_deadline_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    assert_eq!(commit_deadline(&mut setup, dispute_id), DRAW_DELAY + 1);
    let verdict = setup.verdict;
    // A commitment at the deadline itself is accepted.
    set_epoch(&mut setup.test, DRAW_DELAY + 1);
    let first = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[0])
        .expect("drawn juror not found");
    let commitment = vote_commitment(VOTE_PAY, hash_of(1), dispute_id, panel[0]);
    commit_vote(&mut setup, first, dispute_id, commitment);
    // Past the deadline, a commitment is refused, so no juror can wait
    // to see other jurors' revealed votes before committing.
    set_epoch(&mut setup.test, DRAW_DELAY + 2);
    let second = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[1])
        .expect("drawn juror not found");
    let second_secret = second.secret.clone();
    let commitment = vote_commitment(VOTE_PAY, hash_of(2), dispute_id, panel[1]);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&second_secret),
        vec![],
    );
    assert_reject_reason(&reason, "commit window has passed");
}

#[test]
fn reveal_after_reveal_deadline_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    assert_eq!(reveal_deadline(&mut setup, dispute_id), DRAW_DELAY + 2);
    // All three commit inside the window, at the draw epoch.
    for (i, key) in panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn juror not found");
        let commitment = vote_commitment(VOTE_PAY, hash_of((i + 1) as u8), dispute_id, *key);
        commit_vote(&mut setup, juror, dispute_id, commitment);
    }
    // A reveal at the deadline itself is accepted.
    set_epoch(&mut setup.test, DRAW_DELAY + 2);
    let first = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[0])
        .expect("drawn juror not found");
    reveal_vote(&mut setup, first, dispute_id, VOTE_PAY, hash_of(1));
    // Past the deadline, a reveal is refused.
    set_epoch(&mut setup.test, DRAW_DELAY + 3);
    let verdict = setup.verdict;
    let second = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[1])
        .expect("drawn juror not found");
    let second_secret = second.secret.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "reveal_vote",
                args![dispute_id, VOTE_PAY, hash_of(2)],
            )
            .build_and_seal(&second_secret),
        vec![],
    );
    assert_reject_reason(&reason, "reveal window has passed");
}

#[test]
fn appeal_commit_after_deadline_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let (dispute_id, _first, appeal_panel) = dispute_with_appeal_drawn(&mut setup, &jurors);
    let appeal_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    assert_eq!(
        appeal_commit_deadline(&mut setup, dispute_id),
        appeal_epoch + 1
    );
    let verdict = setup.verdict;
    // A commitment at the appeal deadline itself is accepted.
    set_epoch(&mut setup.test, appeal_epoch + 1);
    let first = jurors
        .iter()
        .find(|j| j.pk_bytes() == appeal_panel[0])
        .expect("drawn juror not found");
    let commitment = vote_commitment(VOTE_PAY, hash_of(1), dispute_id, appeal_panel[0]);
    commit_appeal_vote(&mut setup, first, dispute_id, commitment);
    // Past the deadline, an appeal commitment is refused.
    set_epoch(&mut setup.test, appeal_epoch + 2);
    let second = jurors
        .iter()
        .find(|j| j.pk_bytes() == appeal_panel[1])
        .expect("drawn juror not found");
    let second_secret = second.secret.clone();
    let commitment = vote_commitment(VOTE_PAY, hash_of(2), dispute_id, appeal_panel[1]);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_appeal_vote", args![dispute_id, commitment])
            .build_and_seal(&second_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal commit window has passed");
}

#[test]
fn appeal_reveal_after_deadline_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let (dispute_id, _first, appeal_panel) = dispute_with_appeal_drawn(&mut setup, &jurors);
    let appeal_epoch = appeal_draw_epoch(&mut setup, dispute_id);
    assert_eq!(
        appeal_reveal_deadline(&mut setup, dispute_id),
        appeal_epoch + 2
    );
    // All seven commit inside the window, at the appeal draw epoch.
    for (i, key) in appeal_panel.iter().enumerate() {
        let juror = jurors
            .iter()
            .find(|j| j.pk_bytes() == *key)
            .expect("drawn juror not found");
        let commitment = vote_commitment(VOTE_PAY, hash_of((i + 1) as u8), dispute_id, *key);
        commit_appeal_vote(&mut setup, juror, dispute_id, commitment);
    }
    // A reveal at the deadline itself is accepted.
    set_epoch(&mut setup.test, appeal_epoch + 2);
    let first = jurors
        .iter()
        .find(|j| j.pk_bytes() == appeal_panel[0])
        .expect("drawn juror not found");
    reveal_appeal_vote(&mut setup, first, dispute_id, VOTE_PAY, hash_of(1));
    // Past the deadline, an appeal reveal is refused.
    set_epoch(&mut setup.test, appeal_epoch + 3);
    let verdict = setup.verdict;
    let second = jurors
        .iter()
        .find(|j| j.pk_bytes() == appeal_panel[1])
        .expect("drawn juror not found");
    let second_secret = second.secret.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "reveal_appeal_vote",
                args![dispute_id, VOTE_PAY, hash_of(2)],
            )
            .build_and_seal(&second_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal reveal window has passed");
}

#[test]
fn voting_on_settled_dispute_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);
    let verdict = setup.verdict;
    let juror = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[0])
        .expect("drawn juror not found");
    let juror_secret = juror.secret.clone();
    let commitment = vote_commitment(VOTE_PAY, hash_of(0x77), dispute_id, panel[0]);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "dispute already settled");
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "reveal_vote",
                args![dispute_id, VOTE_PAY, hash_of(1)],
            )
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "dispute already settled");
}

#[test]
fn voting_on_closed_dispute_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]);
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let verdict = setup.verdict;
    let juror = jurors
        .iter()
        .find(|j| j.pk_bytes() == panel[2])
        .expect("drawn juror not found");
    let juror_secret = juror.secret.clone();
    let commitment = vote_commitment(VOTE_PAY, hash_of(0x77), dispute_id, panel[2]);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "dispute closed with no ruling");
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "reveal_vote",
                args![dispute_id, VOTE_PAY, hash_of(3)],
            )
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "dispute closed with no ruling");
}

#[test]
fn appeal_refund_returns_to_the_appellant() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    // The payee appeals: the unspent appeal fee must return to the
    // payee, the signer, not to any account a caller could name.
    let payee = setup.payee.clone();
    open_appeal(&mut setup, &payee, dispute_id, APPEAL_FEE);
    let _appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    rule_appeal_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
        ],
    );
    let payer_before = balance(&mut setup.test, &setup.payer);
    let payee_before = balance(&mut setup.test, &setup.payee);
    settle(&mut setup, &payer_secret, dispute_id);
    assert_eq!(dispute_outcome(&mut setup, dispute_id), VOTE_REFUND);
    // Refund: the payer gets the principal less the two first-round
    // juror fees, 490,000. The appeal majority is four, so the
    // leftover appeal fee is 35,000 less four juror fees, 15,000, and
    // it lands with the appellant, the payee.
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + 490_000
    );
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + 15_000
    );
    assert_eq!(vault_balance(&mut setup), 0);
}

#[test]
fn evidence_after_settlement_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settle(&mut setup, &payer_secret, dispute_id);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "submit_evidence",
                args![dispute_id, 0u64, hash_of(0xA1), "ipfs://late", 512u64],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "dispute already settled");
}

#[test]
fn evidence_after_close_rejected() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]);
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "submit_evidence",
                args![dispute_id, 0u64, hash_of(0xA1), "ipfs://late", 512u64],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "dispute closed with no ruling");
}

#[test]
fn close_with_appeal_runs_once_both_rounds_done() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // Two pay votes rule the first round; the third juror stays silent,
    // so the first round is not finished. Its reveal deadline is 4.
    rule_panel_partial(
        &mut setup,
        &jurors,
        dispute_id,
        &panel,
        &[VOTE_PAY, VOTE_PAY],
    );
    let payer = setup.payer.clone();
    open_appeal_with_delay(&mut setup, &payer, dispute_id, APPEAL_FEE, 1);
    let _appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    // The appeal round is fully revealed with no majority: three pay,
    // three refund, one refuse.
    rule_appeal_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUSE,
        ],
    );
    // The appeal round always finishes after the first round's reveal
    // deadline (the appeal draw is at least one epoch after the first
    // draw, and its reveal deadline is two epochs after that), so the
    // first round is final by the time the appeal round is done, and
    // the close runs: the principal returns to the payer and the
    // appeal fee returns in full.
    let payer_before = balance(&mut setup.test, &setup.payer);
    let payee_before = balance(&mut setup.test, &setup.payee);
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    assert_eq!(
        balance(&mut setup.test, &setup.payer),
        payer_before + TOTAL + APPEAL_FEE
    );
    assert_eq!(balance(&mut setup.test, &setup.payee), payee_before);
    assert_eq!(vault_balance(&mut setup), 0);
}

// ----- close timing: finalization strictly after the deadline -----

#[test]
fn close_full_reveal_refused_at_reveal_deadline() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 3);
    let payer_secret = setup.payer.secret.clone();
    // All three reveal with no majority (pay, refund, split), so the
    // round is fully revealed, but the epoch is still the reveal
    // deadline: the close must wait until the window has passed.
    let (dispute_id, _panel) =
        dispute_with_ruling(&mut setup, &jurors, &[VOTE_PAY, VOTE_REFUND, VOTE_SPLIT]);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "first round can still reach a ruling");
    // One epoch past the deadline, the same fully revealed round closes.
    let deadline = reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
}

#[test]
fn close_appeal_full_reveal_refused_at_appeal_deadline() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let (dispute_id, _first, _appeal) = dispute_with_appeal_drawn(&mut setup, &jurors);
    // The appeal round is fully revealed with no majority: three pay,
    // three refund, one refuse. The epoch is the appeal reveal
    // deadline, so the close must wait.
    rule_appeal_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUSE,
        ],
    );
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "close_no_ruling", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "appeal round can still reach a ruling");
    // One epoch past the appeal deadline, the close runs.
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
}

// ----- appeal close slashes first-round failed jurors -----

#[test]
fn appeal_close_slashes_first_round_failed_jurors() {
    let mut setup = make_setup();
    fund(&mut setup, TOTAL);
    let jurors = register_jurors(&mut setup, 7);
    let payer_secret = setup.payer.secret.clone();
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let panel = draw_panel(&mut setup, dispute_id);
    // One juror reveals pay, the other two stay silent: no majority,
    // so the round is replaced and the two silent jurors are recorded
    // as failed.
    rule_panel_partial(&mut setup, &jurors, dispute_id, &panel, &[VOTE_PAY]);
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1);
    let verdict = setup.verdict;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "replace_jury", args![dispute_id, DRAW_DELAY])
            .build_and_seal(&payer_secret),
        vec![],
    );
    // The replacement panel rules pay two to one.
    set_epoch(&mut setup.test, DRAW_DELAY + REVEAL_WINDOW + 1 + DRAW_DELAY);
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
    rule_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    );
    let payer = setup.payer.clone();
    open_appeal(&mut setup, &payer, dispute_id, APPEAL_FEE);
    let _appeal_panel = draw_appeal_panel(&mut setup, dispute_id);
    // The appeal round is fully revealed with no majority: three pay,
    // three refund, one refuse.
    rule_appeal_panel(
        &mut setup,
        &jurors,
        dispute_id,
        &[
            VOTE_PAY,
            VOTE_PAY,
            VOTE_PAY,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUND,
            VOTE_REFUSE,
        ],
    );
    let deadline = appeal_reveal_deadline(&mut setup, dispute_id);
    set_epoch(&mut setup.test, deadline + 1);
    close_no_ruling(&mut setup, &payer_secret, dispute_id);
    let closed: bool = setup.test.call_method(
        setup.verdict,
        "dispute_closed_no_ruling",
        args![dispute_id],
        vec![],
    );
    assert!(closed);
    // The two silent jurors from the original panel were recorded as
    // failed, and the appeal close slashes them: each loses one juror
    // fee of bond. The juror who revealed pay in the original round
    // keeps a full bond, because the replacement discarded that reveal.
    for key in &panel[1..] {
        assert_eq!(juror_bond(&mut setup, *key), BOND_FLOOR - JUROR_FEE);
    }
    assert_eq!(juror_bond(&mut setup, panel[0]), BOND_FLOOR);
    // The two slashed bonds, 10,000 in total, go to the replacement
    // panel's revealers, split by largest remainder: 3,334, 3,333,
    // 3,333 in panel order.
    let expected = [3_334u64, 3_333, 3_333];
    let mut total_claim = 0u64;
    for (i, key) in new_panel.iter().enumerate() {
        let claim = dispute_claim(&mut setup, dispute_id, *key);
        assert_eq!(claim, expected[i]);
        total_claim += claim;
    }
    assert_eq!(total_claim, 2 * JUROR_FEE);
}
