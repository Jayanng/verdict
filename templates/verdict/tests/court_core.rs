// Engine tests for the Verdict court core: juror pool, evidence,
// commit-reveal voting, and panel derivation.
//
// The court core is built on the indirect form: the court never touches
// the escrow vault. These tests run against the real engine. No network
// is involved.

use ootle_byte_type::ToByteType;
use sha2::{Digest, Sha256};
use tari_crypto::ristretto::{RistrettoPublicKey, RistrettoSecretKey};
use tari_engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId};
use tari_ootle_transaction::args;
use tari_template_lib::types::{
    constants::TARI_TOKEN, crypto::RistrettoPublicKeyBytes, Amount, ComponentAddress, Hash32,
    NonFungibleAddress,
};
use tari_template_test_tooling::{
    support::assert_error::assert_reject_reason, xtr_faucet_component, TemplateTest,
};

const TOTAL: u64 = 500_000;
const DRAW_DELAY: u64 = 2;
const MILESTONE_AMOUNTS: [u64; 2] = [200_000, 300_000];
const DEADLINE: u64 = 100;

const VOTE_PAY: u64 = 0;
const VOTE_REFUND: u64 = 1;
const VOTE_SPLIT: u64 = 2;

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

fn make_setup() -> Setup {
    let mut test = new_test();
    let payer = Party::create(&mut test);
    let payee = Party::create(&mut test);
    let stranger = Party::create(&mut test);
    let payer_secret = payer.secret.clone();
    let verdict = create_verdict(&mut test, &payer_secret);
    let escrow = create_escrow(&mut test, verdict, payee.pk_bytes(), &payer_secret);
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

fn bond_floor(test: &mut TemplateTest, verdict: ComponentAddress) -> u64 {
    test.call_method(verdict, "bond_floor", args![], vec![])
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

fn create_jurors(test: &mut TemplateTest, count: usize) -> Vec<Party> {
    (0..count).map(|_| Party::create(test)).collect()
}

/// Create a funded party whose keypair comes from a fixed seed, so the
/// public key is the same on every run. Used for the fixed commitment
/// vector: the expected hash is computed offline from this key.
fn create_funded_party_with_seed(test: &mut TemplateTest, seed: u8) -> Party {
    let (secret, public_key) = test.new_key_pair(seed);
    let public_key_bytes = public_key.to_byte_type();
    let proof = NonFungibleAddress::from_public_key(public_key_bytes);
    // Create the account and fund it from the faucet. Fees stay off:
    // the test harness runs with fees disabled by default, and turning
    // them on here would break later transactions.
    test.disable_fees();
    let account = test.create_account(public_key_bytes, None, vec![proof.clone()]);
    test.execute_expect_success(
        test.transaction()
            .call_method(xtr_faucet_component(), "take", args![account])
            .build_and_seal(&secret),
        vec![proof.clone()],
    );
    Party {
        account,
        proof,
        secret,
        public_key,
    }
}

fn register_jurors(setup: &mut Setup, jurors: &[Party], amount: u64) {
    for juror in jurors {
        register_juror(setup, juror, amount);
    }
}

fn draw_panel(
    setup: &mut Setup,
    dispute_id: u64,
    epoch_hash: Hash32,
) -> Vec<RistrettoPublicKeyBytes> {
    let verdict = setup.verdict;
    let payer_secret = setup.payer.secret.clone();
    set_epoch(&mut setup.test, DRAW_DELAY);
    set_epoch_hash(&mut setup.test, epoch_hash);
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(verdict, "draw_jury", args![dispute_id])
            .build_and_seal(&payer_secret),
        vec![],
    );
    let len: u64 = setup
        .test
        .call_method(verdict, "dispute_panel_len", args![dispute_id], vec![]);
    (0..len)
        .map(|i| {
            setup.test.call_method(
                verdict,
                "dispute_panel_member",
                args![dispute_id, i],
                vec![],
            )
        })
        .collect()
}

/// Open a dispute and draw its panel. Returns the dispute id and the
/// drawn panel. The pool must already hold at least 3 jurors.
fn dispute_with_panel(
    setup: &mut Setup,
    epoch_hash: Hash32,
) -> (u64, Vec<RistrettoPublicKeyBytes>) {
    let payer_secret = setup.payer.secret.clone();
    fund(setup, TOTAL);
    let dispute_id = open_dispute(setup, &payer_secret);
    let panel = draw_panel(setup, dispute_id, epoch_hash);
    (dispute_id, panel)
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

fn submit_evidence(
    setup: &mut Setup,
    party: &Party,
    dispute_id: u64,
    index: u64,
    sha256: Hash32,
    uri: &str,
    bytes_len: u64,
) {
    let verdict = setup.verdict;
    let secret = party.secret.clone();
    setup.test.execute_expect_success(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "submit_evidence",
                args![dispute_id, index, sha256, uri, bytes_len],
            )
            .build_and_seal(&secret),
        vec![],
    );
}

fn reveal_count(setup: &mut Setup, dispute_id: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_reveal_count",
        args![dispute_id],
        vec![],
    )
}

fn evidence_count(setup: &mut Setup, dispute_id: u64, side: u64) -> u64 {
    setup.test.call_method(
        setup.verdict,
        "dispute_evidence_count",
        args![dispute_id, side],
        vec![],
    )
}

// ----- juror pool and bond floor -----

#[test]
fn bond_below_floor_rejected() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    assert!(floor > 0, "bond floor must be positive");
    let juror = Party::create(&mut setup.test);
    let verdict = setup.verdict;
    let juror_account = juror.account;
    let juror_secret = juror.secret.clone();
    let juror_proof = juror.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(juror_account, "withdraw", args![TARI_TOKEN, floor - 1])
            .put_last_instruction_output_on_workspace("bond")
            .call_method(verdict, "register_juror", args![Workspace("bond")])
            .build_and_seal(&juror_secret),
        vec![juror_proof],
    );
    assert_reject_reason(&reason, "bond below the floor");
    let count: u64 = setup
        .test
        .call_method(verdict, "juror_count", args![], vec![]);
    assert_eq!(count, 0, "a rejected registration must not add a juror");
}

#[test]
fn bond_at_floor_accepted() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let juror = Party::create(&mut setup.test);
    let before = balance(&mut setup.test, &juror);
    register_juror(&mut setup, &juror, floor);
    let count: u64 = setup
        .test
        .call_method(setup.verdict, "juror_count", args![], vec![]);
    assert_eq!(count, 1);
    let is_juror: bool =
        setup
            .test
            .call_method(setup.verdict, "is_juror", args![juror.pk_bytes()], vec![]);
    assert!(is_juror);
    let recorded: u64 =
        setup
            .test
            .call_method(setup.verdict, "juror_bond", args![juror.pk_bytes()], vec![]);
    assert_eq!(recorded, floor);
    assert_eq!(balance(&mut setup.test, &juror), before - floor);
}

#[test]
fn duplicate_registration_rejected() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let juror = Party::create(&mut setup.test);
    register_juror(&mut setup, &juror, floor);
    let verdict = setup.verdict;
    let juror_account = juror.account;
    let juror_secret = juror.secret.clone();
    let juror_proof = juror.proof.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(juror_account, "withdraw", args![TARI_TOKEN, floor])
            .put_last_instruction_output_on_workspace("bond")
            .call_method(verdict, "register_juror", args![Workspace("bond")])
            .build_and_seal(&juror_secret),
        vec![juror_proof],
    );
    assert_reject_reason(&reason, "already registered");
    let count: u64 = setup
        .test
        .call_method(verdict, "juror_count", args![], vec![]);
    assert_eq!(count, 1, "one slot per address");
}

// ----- panel derivation -----

#[test]
fn panel_drawn_from_pool_and_stored() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 5);
    register_jurors(&mut setup, &jurors, floor);
    let (_dispute_id, panel) = dispute_with_panel(&mut setup, hash_of(0xD1));
    assert_eq!(panel.len(), 3, "panel holds 3 jurors");
    // Every panel member is a registered juror, and no member repeats.
    let keys: Vec<RistrettoPublicKeyBytes> = jurors.iter().map(|j| j.pk_bytes()).collect();
    for member in &panel {
        assert!(
            keys.contains(member),
            "panel member must be a registered juror"
        );
    }
    let mut sorted = panel.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "panel members must be distinct");
}

// ----- evidence -----

#[test]
fn evidence_over_limit_rejected() {
    let mut setup = make_setup();
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    // The payer posts all 3 items.
    let payer = setup.payer.clone();
    for index in 0..3u64 {
        submit_evidence(
            &mut setup,
            &payer,
            dispute_id,
            index,
            hash_of(0xE0 + index as u8),
            "ipfs://evidence",
            1024,
        );
    }
    assert_eq!(evidence_count(&mut setup, dispute_id, 0), 3);
    // A 4th item (index 3) is rejected.
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "submit_evidence",
                args![dispute_id, 3u64, hash_of(0xE3), "ipfs://evidence", 1024u64],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "out of range");
    assert_eq!(evidence_count(&mut setup, dispute_id, 0), 3);
}

#[test]
fn changing_posted_evidence_rejected() {
    let mut setup = make_setup();
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let payer = setup.payer.clone();
    submit_evidence(
        &mut setup,
        &payer,
        dispute_id,
        0,
        hash_of(0xA1),
        "ipfs://first",
        512,
    );
    let stored: Hash32 = setup.test.call_method(
        setup.verdict,
        "dispute_evidence_hash",
        args![dispute_id, 0u64, 0u64],
        vec![],
    );
    assert_eq!(stored, hash_of(0xA1));
    // Posting a different hash at the same index is rejected.
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "submit_evidence",
                args![dispute_id, 0u64, hash_of(0xB2), "ipfs://changed", 512u64],
            )
            .build_and_seal(&payer_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already posted");
    let stored: Hash32 = setup.test.call_method(
        setup.verdict,
        "dispute_evidence_hash",
        args![dispute_id, 0u64, 0u64],
        vec![],
    );
    assert_eq!(stored, hash_of(0xA1), "posted hash must not change");
}

#[test]
fn stranger_cannot_submit_evidence() {
    let mut setup = make_setup();
    let payer_secret = setup.payer.secret.clone();
    let stranger_secret = setup.stranger.secret.clone();
    fund(&mut setup, TOTAL);
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    let verdict = setup.verdict;
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "submit_evidence",
                args![dispute_id, 0u64, hash_of(0xA1), "ipfs://x", 512u64],
            )
            .build_and_seal(&stranger_secret),
        vec![],
    );
    assert_reject_reason(&reason, "only a party");
    assert_eq!(evidence_count(&mut setup, dispute_id, 0), 0);
    assert_eq!(evidence_count(&mut setup, dispute_id, 1), 0);
}

// ----- commit-reveal voting -----

#[test]
fn unregistered_juror_cannot_commit() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let (dispute_id, _panel) = dispute_with_panel(&mut setup, hash_of(0xD2));
    // The stranger never registered, so they are not drawn.
    let stranger = setup.stranger.clone();
    let stranger_secret = stranger.secret.clone();
    let verdict = setup.verdict;
    let commitment = vote_commitment(VOTE_PAY, hash_of(0x51), dispute_id, stranger.pk_bytes());
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&stranger_secret),
        vec![],
    );
    assert_reject_reason(&reason, "not drawn");
    let count: u64 =
        setup
            .test
            .call_method(verdict, "dispute_commit_count", args![dispute_id], vec![]);
    assert_eq!(count, 0);
}

#[test]
fn undrawn_juror_cannot_commit() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 5);
    register_jurors(&mut setup, &jurors, floor);
    let (dispute_id, panel) = dispute_with_panel(&mut setup, hash_of(0xD3));
    assert_eq!(panel.len(), 3);
    // Find a registered juror who was not drawn.
    let undrawn = jurors
        .iter()
        .find(|j| !panel.contains(&j.pk_bytes()))
        .expect("pool of 5 with a panel of 3 leaves 2 undrawn");
    let undrawn_secret = undrawn.secret.clone();
    let verdict = setup.verdict;
    let commitment = vote_commitment(VOTE_PAY, hash_of(0x52), dispute_id, undrawn.pk_bytes());
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&undrawn_secret),
        vec![],
    );
    assert_reject_reason(&reason, "not drawn");
}

#[test]
fn double_commit_rejected() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let (dispute_id, panel) = dispute_with_panel(&mut setup, hash_of(0xD4));
    let juror = jurors
        .iter()
        .find(|j| panel.contains(&j.pk_bytes()))
        .expect("panel holds registered jurors");
    let salt = hash_of(0x53);
    let commitment = vote_commitment(VOTE_PAY, salt, dispute_id, juror.pk_bytes());
    commit_vote(&mut setup, juror, dispute_id, commitment);
    // A second commit from the same juror is rejected.
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "commit_vote", args![dispute_id, commitment])
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "already committed");
}

#[test]
fn reveal_not_matching_commit_rejected() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let (dispute_id, panel) = dispute_with_panel(&mut setup, hash_of(0xD5));
    let juror = jurors
        .iter()
        .find(|j| panel.contains(&j.pk_bytes()))
        .expect("panel holds registered jurors");
    let salt = hash_of(0x54);
    let commitment = vote_commitment(VOTE_PAY, salt, dispute_id, juror.pk_bytes());
    commit_vote(&mut setup, juror, dispute_id, commitment);
    let verdict = setup.verdict;
    let juror_secret = juror.secret.clone();
    // Revealing a different vote with the same salt does not match.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(verdict, "reveal_vote", args![dispute_id, VOTE_REFUND, salt])
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "does not match");
    // Revealing the same vote with a different salt does not match.
    let reason = setup.test.execute_expect_failure(
        setup
            .test
            .transaction()
            .call_method(
                verdict,
                "reveal_vote",
                args![dispute_id, VOTE_PAY, hash_of(0x99)],
            )
            .build_and_seal(&juror_secret),
        vec![],
    );
    assert_reject_reason(&reason, "does not match");
    assert_eq!(reveal_count(&mut setup, dispute_id), 0);
}

#[test]
fn commit_without_reveal_does_not_count_as_vote() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let (dispute_id, panel) = dispute_with_panel(&mut setup, hash_of(0xD6));
    let juror = jurors
        .iter()
        .find(|j| panel.contains(&j.pk_bytes()))
        .expect("panel holds registered jurors");
    let salt = hash_of(0x55);
    let commitment = vote_commitment(VOTE_SPLIT, salt, dispute_id, juror.pk_bytes());
    commit_vote(&mut setup, juror, dispute_id, commitment);
    // The commit alone records no vote: the reveal count stays 0.
    assert_eq!(reveal_count(&mut setup, dispute_id), 0);
    // After the reveal, the vote is recorded.
    reveal_vote(&mut setup, juror, dispute_id, VOTE_SPLIT, salt);
    assert_eq!(reveal_count(&mut setup, dispute_id), 1);
    let recorded: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_reveal",
        args![dispute_id, juror.pk_bytes()],
        vec![],
    );
    assert_eq!(recorded, VOTE_SPLIT);
}

// ----- happy path -----

#[test]
fn full_dispute_flow_panel_and_votes_recorded() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    assert_eq!(floor, 30_000, "bond floor is twice the fee times the panel");
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    let count: u64 = setup
        .test
        .call_method(setup.verdict, "juror_count", args![], vec![]);
    assert_eq!(count, 3);

    // Open the dispute, both sides post evidence.
    let payer = setup.payer.clone();
    let payee = setup.payee.clone();
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    submit_evidence(
        &mut setup,
        &payer,
        dispute_id,
        0,
        hash_of(0xC1),
        "ipfs://payer-work",
        2048,
    );
    submit_evidence(
        &mut setup,
        &payee,
        dispute_id,
        0,
        hash_of(0xC2),
        "ipfs://payee-proof",
        4096,
    );
    assert_eq!(evidence_count(&mut setup, dispute_id, 0), 1);
    assert_eq!(evidence_count(&mut setup, dispute_id, 1), 1);

    // Draw the panel in the locked epoch.
    let panel = draw_panel(&mut setup, dispute_id, hash_of(0xD7));
    assert_eq!(panel.len(), 3);
    let drawn: bool =
        setup
            .test
            .call_method(setup.verdict, "dispute_drawn", args![dispute_id], vec![]);
    assert!(drawn);

    // All three drawn jurors commit and reveal. Two vote pay, one votes
    // refund, so the majority is pay.
    let votes = [VOTE_PAY, VOTE_PAY, VOTE_REFUND];
    for (i, juror) in jurors.iter().enumerate() {
        assert!(
            panel.contains(&juror.pk_bytes()),
            "with 3 jurors and a panel of 3, every juror is drawn"
        );
        let salt = hash_of(0x60 + i as u8);
        let commitment = vote_commitment(votes[i], salt, dispute_id, juror.pk_bytes());
        commit_vote(&mut setup, juror, dispute_id, commitment);
    }
    assert_eq!(
        setup.test.call_method::<u64>(
            setup.verdict,
            "dispute_commit_count",
            args![dispute_id],
            vec![],
        ),
        3
    );
    for (i, juror) in jurors.iter().enumerate() {
        let salt = hash_of(0x60 + i as u8);
        reveal_vote(&mut setup, juror, dispute_id, votes[i], salt);
    }
    assert_eq!(reveal_count(&mut setup, dispute_id), 3);
    for (i, juror) in jurors.iter().enumerate() {
        let recorded: u64 = setup.test.call_method(
            setup.verdict,
            "dispute_reveal",
            args![dispute_id, juror.pk_bytes()],
            vec![],
        );
        assert_eq!(recorded, votes[i]);
    }
}

// ----- frozen pool -----

#[test]
fn late_registration_does_not_change_panel() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 3);
    register_jurors(&mut setup, &jurors, floor);
    // Open the dispute with only 3 jurors registered.
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    // Register 2 more jurors AFTER the dispute opened.
    let late = create_jurors(&mut setup.test, 2);
    register_jurors(&mut setup, &late, floor);
    // Draw the panel. It must come from the frozen pool of 3, not the
    // live pool of 5.
    let panel = draw_panel(&mut setup, dispute_id, hash_of(0xD9));
    assert_eq!(panel.len(), 3);
    let keys: Vec<RistrettoPublicKeyBytes> = jurors.iter().map(|j| j.pk_bytes()).collect();
    for member in &panel {
        assert!(
            keys.contains(member),
            "panel must come from the pool at open time"
        );
    }
    for late_juror in &late {
        assert!(
            !panel.contains(&late_juror.pk_bytes()),
            "a late juror must not be drawn"
        );
    }
    // The snapshot still holds only the 3 original jurors.
    let pool_len: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_eligible_pool_len",
        args![dispute_id],
        vec![],
    );
    assert_eq!(pool_len, 3);
}

#[test]
fn snapshot_getters_return_pool_at_open_time() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    let jurors = create_jurors(&mut setup.test, 4);
    register_jurors(&mut setup, &jurors, floor);
    let payer_secret = setup.payer.secret.clone();
    fund(&mut setup, TOTAL);
    let dispute_id = open_dispute(&mut setup, &payer_secret);
    // The round starts at 0 and the snapshot holds the 4 jurors in
    // registration order.
    let round: u64 =
        setup
            .test
            .call_method(setup.verdict, "dispute_round", args![dispute_id], vec![]);
    assert_eq!(round, 0);
    let pool_len: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_eligible_pool_len",
        args![dispute_id],
        vec![],
    );
    assert_eq!(pool_len, 4);
    for (i, juror) in jurors.iter().enumerate() {
        let member: RistrettoPublicKeyBytes = setup.test.call_method(
            setup.verdict,
            "dispute_eligible_pool_member",
            args![dispute_id, i as u64],
            vec![],
        );
        assert_eq!(
            member,
            juror.pk_bytes(),
            "snapshot keeps registration order"
        );
    }
    // A juror who registers after the dispute opens is not in the snapshot.
    let late = Party::create(&mut setup.test);
    register_juror(&mut setup, &late, floor);
    let pool_len: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_eligible_pool_len",
        args![dispute_id],
        vec![],
    );
    assert_eq!(pool_len, 4, "the snapshot does not grow after open");
}

// ----- fixed commitment vector -----
//
// The commitment is SHA-256 of the concatenation, in this order:
//   vote as 8 bytes little-endian
//   salt as 32 raw bytes
//   dispute id as 8 bytes little-endian
//   juror public key as 32 raw bytes
// A wallet implementer can reproduce this exactly. The expected hash
// below was computed offline from the fixed inputs with an independent
// tool, not from the template, so this test pins the encoding.

fn hex32(hex: &str) -> Hash32 {
    let bytes = hex.as_bytes();
    assert_eq!(bytes.len(), 64, "a 32-byte hash is 64 hex chars");
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (hex_val(bytes[2 * i]) << 4) | hex_val(bytes[2 * i + 1]);
    }
    Hash32::from(out)
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => panic!("not a hex digit"),
    }
}

#[test]
fn commitment_encoding_fixed_vector() {
    let mut setup = make_setup();
    let floor = bond_floor(&mut setup.test, setup.verdict);
    // The fixed juror comes from seed 42, so its public key is the
    // same on every run. Two more jurors fill the panel of 3.
    let fixed_juror = create_funded_party_with_seed(&mut setup.test, 42);
    let others = create_jurors(&mut setup.test, 2);
    register_juror(&mut setup, &fixed_juror, floor);
    register_jurors(&mut setup, &others, floor);
    let (dispute_id, panel) = dispute_with_panel(&mut setup, hash_of(0xD8));
    assert_eq!(dispute_id, 0, "first dispute in a fresh setup is id 0");
    assert_eq!(panel.len(), 3);
    assert!(panel.contains(&fixed_juror.pk_bytes()));

    // Fixed inputs for the vector: vote pay, salt of 0x5a bytes,
    // dispute id 0, and the fixed juror key from seed 42.
    let vote = VOTE_PAY;
    let salt = Hash32::from([0x5au8; 32]);
    // Expected SHA-256 of vote LE || salt || dispute id LE || juror,
    // computed offline with an independent tool from the fixed inputs.
    let expected = hex32("0c3fd3ba580798216780f4135257c7ffb8818290dcabe3f94964c311a568f2fa");
    // Commit with the fixed hash, then reveal with the fixed inputs.
    // The reveal only succeeds if the template computes the same hash.
    commit_vote(&mut setup, &fixed_juror, dispute_id, expected);
    reveal_vote(&mut setup, &fixed_juror, dispute_id, vote, salt);
    assert_eq!(reveal_count(&mut setup, dispute_id), 1);
    let recorded: u64 = setup.test.call_method(
        setup.verdict,
        "dispute_reveal",
        args![dispute_id, fixed_juror.pk_bytes()],
        vec![],
    );
    assert_eq!(recorded, vote);
}
