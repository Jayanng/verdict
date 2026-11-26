// Engine tests for Verdict (indirect form).
//
// (a) Verdict calls escrow.execute_ruling, the escrow moves its own vault,
//     and the ruling executes exactly once.
// (b) A stranger calling execute_ruling / freeze_for_dispute directly is
//     rejected by the access rule AND by the in-method assert (each layer
//     proven separately via the rule-open twin).
// (c) Owned-twin positive control: the owned twin's owner bypasses the
//     caller rule, while the ownerless twin refuses the same direct call.
// (d) The template reads the current epoch hash in-template, and the
//     future-epoch draw commitment (E* = open_epoch + K) compiles and
//     enforces its single-epoch window.
//
// Both templates are loaded into one harness so the cross-component calls
// run against the real engine. No network is involved.

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
const PAYEE_SHARE: u64 = 300_000;
const PAYER_SHARE: u64 = TOTAL - PAYEE_SHARE;
const DRAW_DELAY: u64 = 2;
const MILESTONE_AMOUNTS: [u64; 2] = [200_000, 300_000];
const DEADLINE: u64 = 100;
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
    // Cargo runs tests with CWD = the verdict package dir, so the workspace
    // root is ../.. and both template crates load from there.
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

/// Create an escrow with the given constructor ("new", "new_owned" or
/// "new_rule_open"), signed by `creator`. The arbitrator address is the
/// verdict component passed in.
fn create_escrow(
    test: &mut TemplateTest,
    ctor: &str,
    arbitrator: ComponentAddress,
    payee_pk: RistrettoPublicKeyBytes,
    creator: &RistrettoSecretKey,
) -> ComponentAddress {
    let template = test.get_template_address("MilestoneEscrow");
    test.execute_expect_success(
        test.transaction()
            .call_function(
                template,
                ctor,
                args![arbitrator, payee_pk, MILESTONE_AMOUNTS.to_vec(), DEADLINE],
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
    let escrow = create_escrow(&mut test, ctor, verdict, payee.pk_bytes(), &payer_secret);
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
    fund_on(setup, setup.escrow, amount);
}

fn fund_on(setup: &mut Setup, escrow: ComponentAddress, amount: u64) {
    let payer_account = setup.payer.account;
    let payer_secret = setup.payer.secret.clone();
    let payer_proof = setup.payer.proof.clone();
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

fn open_dispute(setup: &mut Setup, signer: &RistrettoSecretKey) -> u64 {
    open_dispute_on(setup, setup.escrow, signer)
}

fn open_dispute_on(
    setup: &mut Setup,
    escrow: ComponentAddress,
    signer: &RistrettoSecretKey,
) -> u64 {
    let verdict = setup.verdict;
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

/// Settle a dispute. Settle runs only once the reveal window has
/// passed, so the epoch moves to the reveal deadline first.
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

fn set_epoch_hash(test: &mut TemplateTest, hash: Hash32) {
    test.set_virtual_substate(
        VirtualSubstateId::CurrentEpochHash,
        VirtualSubstate::CurrentEpochHash(hash),
    );
}

fn hash_of(byte: u8) -> Hash32 {
    Hash32::from([byte; 32])
}

// ----- (a) indirect execution -----

#[test]
fn verdict_settle_moves_escrow_vault_exactly_once() {
    let mut setup = make_setup("new");
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);

    fund(&mut setup, TOTAL);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);

    let jurors = register_jurors(&mut setup, 3);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    assert_eq!(dispute_id, 0);
    let frozen: bool = setup
        .test
        .call_method(setup.escrow, "is_frozen", args![], vec![]);
    assert!(
        frozen,
        "open_dispute must freeze the escrow via its own method"
    );

    // Two jurors rule pay, one rules refund: the ruling is pay, the two
    // majority jurors are owed one fee each from the principal, and the
    // payer keeps nothing. The amount is derived from the votes, never
    // supplied by the caller.
    rule_dispute(
        &mut setup,
        &jurors,
        dispute_id,
        [VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    );
    settle(&mut setup, &payer_secret, dispute_id);

    // Exactly the ruled amount moved to the payee, the juror fees stayed
    // in the court's fee vault, and nothing is left in the escrow vault.
    // Conservation is exact: payee gain + juror fees + payer refund
    // (0) == funded total.
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + TOTAL - 2 * JUROR_FEE
    );
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before - TOTAL);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(executed);
    let settled: bool =
        setup
            .test
            .call_method(setup.verdict, "dispute_settled", args![dispute_id], vec![]);
    assert!(settled);

    // Second settle of the same dispute must fail: the ruling executes once.
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
    // Nothing moved on the failed second settle.
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + TOTAL - 2 * JUROR_FEE
    );
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), 0);
}

// ----- (b) stranger rejection, both layers -----

#[test]
fn stranger_direct_call_rejected_by_access_rule() {
    let mut setup = make_setup("new");
    let payer_secret = setup.payer.secret.clone();
    let stranger_secret = setup.stranger.secret.clone();
    fund(&mut setup, TOTAL);

    // Direct freeze_for_dispute by a stranger: the caller-component rule
    // must reject it before the method body ever runs.
    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&stranger_secret),
        vec![],
    );
    eprintln!("rule-layer freeze reject reason: {reason}");
    assert_reject_reason(
        &reason,
        "Access Denied: call component method 'freeze_for_dispute'",
    );

    // Direct execute_ruling by a stranger: same rule, same rejection.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "execute_ruling", args![0u64, PAYEE_SHARE])
            .build_and_seal(&stranger_secret),
        vec![],
    );
    eprintln!("rule-layer ruling reject reason: {reason}");
    assert_reject_reason(
        &reason,
        "Access Denied: call component method 'execute_ruling'",
    );

    // The escrow is untouched: never frozen, vault intact.
    let frozen: bool = setup
        .test
        .call_method(setup.escrow, "is_frozen", args![], vec![]);
    assert!(!frozen);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);

    // The payer (creator, but NOT the owner: OwnerRule::None) is refused
    // the same direct call, proving no owner-bypass path exists here.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(
        &reason,
        "Access Denied: call component method 'freeze_for_dispute'",
    );
}

#[test]
fn stranger_direct_call_rejected_by_in_method_assert() {
    // Rule-open twin: allow-all rules, so only the in-method caller assert
    // stands between a stranger and the vault.
    let mut setup = make_setup("new_rule_open");
    let stranger_secret = setup.stranger.secret.clone();
    fund(&mut setup, TOTAL);

    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&stranger_secret),
        vec![],
    );
    eprintln!("assert-layer freeze reject reason: {reason}");
    // The badge assert fails with the same AccessDenied code as a rule
    // rejection, but a distinct message: the call "does not carry the
    // badge" naming the arbitrator. It must NOT be the rule's message
    // ("Access Denied: call component method ..."), because the twin's
    // rules are wide open and only the assert stands.
    assert_reject_reason(&reason, "does not carry the badge");
    assert!(
        !reason
            .to_string()
            .contains("Access Denied: call component method"),
        "rule-open twin must not fail on an access rule, got: {reason}"
    );

    let frozen: bool = setup
        .test
        .call_method(setup.escrow, "is_frozen", args![], vec![]);
    assert!(!frozen);
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), TOTAL);
}

// ----- (c) owned-twin positive control -----

#[test]
fn owned_twin_owner_bypasses_caller_rule() {
    // Owned twin: the payer created it, so the payer is its owner. The
    // owner bypasses the caller-component rule, and the twin's assert is
    // off, so the direct calls must SUCCEED.
    let mut setup = make_setup("new_owned");
    let payer_secret = setup.payer.secret.clone();
    let payee_before = balance(&mut setup.test, &setup.payee);
    let payer_before = balance(&mut setup.test, &setup.payer);
    fund(&mut setup, TOTAL);

    let escrow = setup.escrow;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let frozen: bool = setup
        .test
        .call_method(setup.escrow, "is_frozen", args![], vec![]);
    assert!(frozen, "owner bypass must let the owner freeze directly");

    // Owner executes the ruling directly, routing the returned payee
    // bucket into the payee's account in the same transaction.
    let payee_account = setup.payee.account;
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(escrow, "execute_ruling", args![0u64, PAYEE_SHARE])
            .put_last_instruction_output_on_workspace("ruling")
            .call_method(payee_account, "deposit", args![Workspace("ruling")])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_eq!(
        balance(&mut setup.test, &setup.payee),
        payee_before + PAYEE_SHARE
    );
    assert_eq!(balance(&mut setup.test, &setup.payer), payer_before - TOTAL);
    // Direct execute_ruling moves only the payee share; the remainder
    // stays in the vault until withdraw_remainder routes it to the payer.
    assert_eq!(vault_balance(&mut setup.test, setup.escrow), PAYER_SHARE);
    let executed: bool = setup
        .test
        .call_method(setup.escrow, "ruling_executed", args![], vec![]);
    assert!(executed);
}

#[test]
fn ownerless_twin_refuses_same_direct_call() {
    // Same direct calls as the owned twin, same signer, but the escrow is
    // ownerless: no owner-bypass path exists, so the rule refuses.
    let mut setup = make_setup("new");
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);

    let escrow = setup.escrow;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(escrow, "freeze_for_dispute", args![0u64])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "Access");
    let frozen: bool = setup
        .test
        .call_method(setup.escrow, "is_frozen", args![], vec![]);
    assert!(!frozen);
}

// ----- (d) epoch hash read + future-epoch commitment -----

#[test]
fn template_reads_current_epoch_hash() {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);

    // Default seed is epoch 0 with the empty hash.
    let epoch: u64 = test.call_method(verdict, "current_epoch", args![], vec![]);
    assert_eq!(epoch, 0);
    let hash: Hash32 = test.call_method(verdict, "current_epoch_hash", args![], vec![]);
    assert_eq!(hash, Hash32::from([0u8; 32]));

    // Inject a known hash and confirm the in-template read tracks it.
    set_epoch_hash(&mut test, hash_of(0xAB));
    let hash: Hash32 = test.call_method(verdict, "current_epoch_hash", args![], vec![]);
    assert_eq!(hash, hash_of(0xAB));

    // Move to another epoch with a different hash: the read follows.
    set_epoch(&mut test, 5);
    set_epoch_hash(&mut test, hash_of(0x5E));
    let epoch: u64 = test.call_method(verdict, "current_epoch", args![], vec![]);
    assert_eq!(epoch, 5);
    let hash: Hash32 = test.call_method(verdict, "current_epoch_hash", args![], vec![]);
    assert_eq!(hash, hash_of(0x5E));
}

#[test]
fn future_epoch_draw_commitment_enforced() {
    let mut setup = make_setup("new");
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);

    // Open at epoch 0 with K = 2: the draw locks to epoch 2, and the
    // opening hash (the predictable current one) is recorded.
    set_epoch_hash(&mut setup.test, hash_of(0x01));
    let _jurors = register_jurors(&mut setup, 3);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let draw_epoch: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_epoch",
        args![dispute_id],
        vec![],
    );
    assert_eq!(draw_epoch, 2);
    let open_hash: Hash32 = setup.test.call_method(
        setup.verdict,
        "dispute_open_epoch_hash",
        args![dispute_id],
        vec![],
    );
    assert_eq!(open_hash, hash_of(0x01));

    // Draw before E* fails: epoch 0, then epoch 1.
    let verdict = setup.verdict;
    for epoch in [0u64, 1] {
        set_epoch(&mut setup.test, epoch);
        let reason = setup.test.execute_expect_failure(
            setup
                .test
                .transaction()
                .call_method(verdict, "draw_jury", args![dispute_id])
                .build_and_seal(&payer_secret),
            vec![],
        );
        assert_reject_reason(&reason, "locked draw epoch");
    }

    // At E* = 2 the draw executes and reads that epoch's (future, then
    // unknowable) hash in-template.
    set_epoch(&mut setup.test, 2);
    set_epoch_hash(&mut setup.test, hash_of(0xE2));
    let drawn: Hash32 = setup
        .test
        .call_method(verdict, "draw_jury", args![dispute_id], vec![]);
    assert_eq!(drawn, hash_of(0xE2));
    let stored: Hash32 = setup.test.call_method(
        setup.verdict,
        "dispute_draw_hash",
        args![dispute_id],
        vec![],
    );
    assert_eq!(stored, hash_of(0xE2));

    // Second draw in the same epoch fails: one draw per dispute.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already drawn");

    // A second dispute on a second escrow, opened at epoch 2, locks to
    // epoch 4; drawing it at epoch 3 (before E*) fails, proving the
    // window is per-dispute. One escrow holds one dispute.
    let payee_pk = setup.payee.pk_bytes();
    let escrow2 = create_escrow(
        &mut setup.test,
        "new",
        setup.verdict,
        payee_pk,
        &payer_secret,
    );
    fund_on(&mut setup, escrow2, TOTAL);
    let dispute2 = open_dispute_on(&mut setup, escrow2, &payer_secret);
    let draw_epoch2: u64 =
        setup
            .test
            .call_method(setup.verdict, "dispute_draw_epoch", args![dispute2], vec![]);
    assert_eq!(draw_epoch2, 4);
    set_epoch(&mut setup.test, 3);
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "draw_jury", args![dispute2])
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "locked draw epoch");
}
