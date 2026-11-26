// Tests for web/src/verdict.js and the generated sample data.
// Run from web/: node --test tests/
// These are logic tests for the proof page. The simulation parity suite
// lives in tests/parity.test.js and proves the simulation in
// src/simulation.js agrees with this mirror and with the template rules.

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  PANEL_SIZE,
  APPEAL_PANEL_SIZE,
  JUROR_FEE,
  BOND_FLOOR,
  APPEAL_FEE,
  SLASH,
  MIN_DRAW_DELAY,
  REVEAL_WINDOW,
  DRAW_DOMAIN_FIRST,
  DRAW_DOMAIN_APPEAL,
  MAX_RESCHEDULES,
  VOTE_PAY,
  VOTE_REFUND,
  VOTE_SPLIT,
  VOTE_REFUSE,
  hexToBytes,
  bytesToHex,
  sha256Hex,
  u64ToLeBytes,
  voteCommitmentBytes,
  derivePanel,
  majorityOutcome,
  settleAmounts,
  splitLargestRemainder,
  callerRuleCheck,
} from "../src/verdict.js";
import { SAMPLE } from "../src/sample-data.js";

describe("template constants", () => {
  test("panel size, juror fee, and bond floor match the template", () => {
    assert.equal(PANEL_SIZE, 3);
    assert.equal(APPEAL_PANEL_SIZE, 7);
    assert.equal(JUROR_FEE, 5000);
    assert.equal(BOND_FLOOR, 30000);
    assert.equal(BOND_FLOOR, 2 * JUROR_FEE * PANEL_SIZE);
    assert.equal(APPEAL_FEE, 35000);
    assert.equal(SLASH, 5000);
    assert.equal(MIN_DRAW_DELAY, 1);
    assert.equal(REVEAL_WINDOW, 2);
    assert.equal(DRAW_DOMAIN_FIRST, 0);
    assert.equal(DRAW_DOMAIN_APPEAL, 1);
  });
});

describe("sha256Hex", () => {
  test("matches known vectors, including a multi-block input", () => {
    assert.equal(
      sha256Hex(""),
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    assert.equal(
      sha256Hex("abc"),
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    const long = "verdict-sample-evidence-text-".repeat(10);
    assert.equal(sha256Hex(long), createHash("sha256").update(long, "utf8").digest("hex"));
  });
});

describe("byte helpers", () => {
  test("hex round trip", () => {
    const hex = "0018ff" + "ab".repeat(29);
    assert.equal(bytesToHex(hexToBytes(hex)), hex);
  });

  test("u64 little-endian encoding", () => {
    assert.equal(bytesToHex(u64ToLeBytes(0)), "0000000000000000");
    assert.equal(bytesToHex(u64ToLeBytes(1)), "0100000000000000");
    assert.equal(bytesToHex(u64ToLeBytes(13372)), "3c34000000000000");
  });

  test("vote commitment bytes are vote, salt, dispute id, juror, in order", () => {
    const bytes = voteCommitmentBytes(VOTE_PAY, "00".repeat(32), 0, "ff".repeat(32));
    assert.equal(bytes.length, 80);
    assert.equal(bytesToHex(bytes.slice(0, 8)), "0000000000000000");
    assert.equal(bytesToHex(bytes.slice(8, 40)), "00".repeat(32));
    assert.equal(bytesToHex(bytes.slice(40, 48)), "0000000000000000");
    assert.equal(bytesToHex(bytes.slice(48, 80)), "ff".repeat(32));
  });
});

describe("derivePanel", () => {
  const pool = SAMPLE.pool;
  const drawHash = SAMPLE.disputes[0].drawHash;

  test("returns three distinct jurors, in range, deterministic", () => {
    const panel = derivePanel(pool, drawHash, 0, 0);
    assert.equal(panel.length, 3);
    assert.equal(new Set(panel).size, 3);
    for (const i of panel) assert.ok(i >= 0 && i < pool.length);
    assert.deepEqual(derivePanel(pool, drawHash, 0, 0), panel);
  });

  test("changing one draw input changes the panel", () => {
    const base = derivePanel(pool, drawHash, 0, 0);
    const changedHash = derivePanel(pool, "00" + drawHash.slice(2), 0, 0);
    const changedId = derivePanel(pool, drawHash, 1, 0);
    const changedRound = derivePanel(pool, drawHash, 0, 1);
    assert.ok(
      JSON.stringify(changedHash) !== JSON.stringify(base) ||
        JSON.stringify(changedId) !== JSON.stringify(base) ||
        JSON.stringify(changedRound) !== JSON.stringify(base),
      "at least one changed input must change the panel",
    );
    assert.notDeepEqual(changedRound, base);
  });

  test("changing the draw domain changes the panel", () => {
    const first = derivePanel(pool, drawHash, 0, 0, DRAW_DOMAIN_FIRST, PANEL_SIZE);
    const appeal = derivePanel(pool, drawHash, 0, 0, DRAW_DOMAIN_APPEAL, APPEAL_PANEL_SIZE);
    assert.notDeepEqual(appeal.slice(0, 3), first);
    assert.notDeepEqual(appeal, first);
  });

  test("a pool smaller than the panel is refused", () => {
    assert.throws(() => derivePanel(pool.slice(0, 2), drawHash, 0, 0), /not enough eligible jurors/);
    assert.throws(() => derivePanel([], drawHash, 0, 0), /not enough eligible jurors/);
    assert.throws(
      () => derivePanel(pool.slice(0, 6), drawHash, 0, 0, DRAW_DOMAIN_APPEAL, APPEAL_PANEL_SIZE),
      /not enough eligible jurors/,
    );
  });
});

describe("majorityOutcome", () => {
  test("two of three is a majority", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY, VOTE_PAY, VOTE_REFUND]), {
      vote: VOTE_PAY,
      count: 2,
    });
  });

  test("one of each is no majority", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY, VOTE_REFUND, VOTE_SPLIT]), {
      vote: null,
      count: 1,
    });
  });

  test("one revealed vote for pay returns no ruling: the quorum is not met", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY], PANEL_SIZE), {
      vote: null,
      count: 0,
    });
  });

  test("two refuse votes return no ruling: refuse never counts", () => {
    assert.deepEqual(majorityOutcome([VOTE_REFUSE, VOTE_REFUSE], PANEL_SIZE), {
      vote: null,
      count: 0,
    });
    assert.deepEqual(majorityOutcome([VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY], PANEL_SIZE), {
      vote: null,
      count: 1,
    });
  });

  test("three reveals on a panel of seven return no ruling: the quorum is not met", () => {
    assert.deepEqual(
      majorityOutcome([VOTE_PAY, VOTE_PAY, VOTE_PAY], APPEAL_PANEL_SIZE),
      { vote: null, count: 0 },
    );
  });

  test("a vote that does not exist is refused, not counted", () => {
    assert.throws(() => majorityOutcome([99, 99, 99], PANEL_SIZE), /one of pay, refund, split, or refuse/);
    assert.throws(() => majorityOutcome([VOTE_PAY, 4, VOTE_PAY], PANEL_SIZE), /one of pay, refund, split, or refuse/);
    assert.throws(() => majorityOutcome([VOTE_PAY, -1, VOTE_PAY], PANEL_SIZE), /one of pay, refund, split, or refuse/);
    assert.throws(() => majorityOutcome([VOTE_PAY, 1.5, VOTE_PAY], PANEL_SIZE), /one of pay, refund, split, or refuse/);
  });

  test("a panel size that is not a non-negative whole number is refused", () => {
    assert.throws(() => majorityOutcome([VOTE_PAY, VOTE_PAY], -1), /non-negative whole number/);
    assert.throws(() => majorityOutcome([VOTE_PAY, VOTE_PAY], 1.5), /non-negative whole number/);
    assert.throws(() => majorityOutcome([VOTE_PAY, VOTE_PAY], Number.NaN), /non-negative whole number/);
    assert.throws(() => majorityOutcome([VOTE_PAY, VOTE_PAY], "3"), /non-negative whole number/);
    assert.throws(() => majorityOutcome("pay", PANEL_SIZE), /array of votes/);
  });

  test("an undrawn panel of size 0 returns no ruling, not a refusal", () => {
    assert.deepEqual(majorityOutcome([], 0), { vote: null, count: 0 });
    assert.deepEqual(majorityOutcome([VOTE_PAY, VOTE_PAY], 0), { vote: null, count: 0 });
  });
});

describe("splitLargestRemainder", () => {
  test("parts always sum to the total exactly", () => {
    assert.deepEqual(splitLargestRemainder(100, [1, 1, 1]), [34, 33, 33]);
    assert.deepEqual(splitLargestRemainder(500000, [1, 1]), [250000, 250000]);
    assert.deepEqual(splitLargestRemainder(7, [2, 1]), [5, 2]);
    const parts = splitLargestRemainder(999983, [3, 2, 1]);
    assert.equal(parts.reduce((a, b) => a + b, 0), 999983);
  });

  test("all empty weights are rejected", () => {
    assert.throws(() => splitLargestRemainder(100, [0, 0]), /weights must sum to more than 0/);
  });
});

describe("settleAmounts", () => {
  test("pay routes the principal less the majority fees to the payee", () => {
    assert.deepEqual(settleAmounts(500000, VOTE_PAY, 2), {
      fees: 10000,
      payeeAmount: 490000,
      payerAmount: 10000,
      payerAmountNet: 0,
    });
  });

  test("refund routes the principal less the majority fees to the payer", () => {
    assert.deepEqual(settleAmounts(300000, VOTE_REFUND, 2), {
      fees: 10000,
      payeeAmount: 0,
      payerAmount: 300000,
      payerAmountNet: 290000,
    });
  });

  test("split divides the remainder by largest remainder, odd micro-Tari to the payee", () => {
    assert.deepEqual(settleAmounts(100001, VOTE_SPLIT, 2), {
      fees: 10000,
      payeeAmount: 45001,
      payerAmount: 55000,
      payerAmountNet: 45000,
    });
    assert.deepEqual(settleAmounts(500000, VOTE_SPLIT, 2), {
      fees: 10000,
      payeeAmount: 245000,
      payerAmount: 255000,
      payerAmountNet: 245000,
    });
  });

  test("fees are capped at a small principal", () => {
    assert.deepEqual(settleAmounts(10000, VOTE_PAY, 2), {
      fees: 10000,
      payeeAmount: 0,
      payerAmount: 10000,
      payerAmountNet: 0,
    });
  });

  test("refuse is not a ruled outcome", () => {
    assert.throws(() => settleAmounts(500000, VOTE_REFUSE, 2), /needs a ruled outcome/);
  });
});

describe("callerRuleCheck", () => {
  test("the court is accepted, a stranger is refused", () => {
    const court = "component-court";
    assert.equal(callerRuleCheck(court, court).allowed, true);
    const stranger = callerRuleCheck("component-stranger", court);
    assert.equal(stranger.allowed, false);
    assert.match(stranger.reason, /refuses/);
  });

  test("two missing identities are refused, not accepted as equal", () => {
    const court = "component-court";
    assert.equal(callerRuleCheck(undefined, undefined).allowed, false);
    assert.equal(callerRuleCheck(null, null).allowed, false);
    assert.equal(callerRuleCheck("", "").allowed, false);
    assert.equal(callerRuleCheck(undefined, court).allowed, false);
    assert.equal(callerRuleCheck(court, undefined).allowed, false);
    assert.equal(callerRuleCheck("", court).allowed, false);
    assert.equal(callerRuleCheck(court, "").allowed, false);
    assert.match(callerRuleCheck(undefined, undefined).reason, /missing/);
  });
});

describe("sample data consistency", () => {
  test("every sample panel is really derived from its draw hash", () => {
    for (const dispute of SAMPLE.disputes) {
      const panel = derivePanel(SAMPLE.pool, dispute.drawHash, dispute.id, dispute.round);
      assert.deepEqual(panel, dispute.panelIndexes, `dispute ${dispute.id} panel`);
    }
  });

  test("every sample commitment is really SHA-256 of vote, salt, dispute, juror", () => {
    for (const dispute of SAMPLE.disputes) {
      for (const commit of dispute.commits) {
        const expected = sha256Hex(
          voteCommitmentBytes(commit.vote, commit.salt, dispute.id, commit.juror),
        );
        assert.equal(commit.commitment, expected, `dispute ${dispute.id} commitment`);
      }
    }
  });

  test("every sample evidence hash is really SHA-256 of its bytes", () => {
    for (const dispute of SAMPLE.disputes) {
      for (const item of dispute.evidence) {
        const expected = createHash("sha256").update(item.text, "utf8").digest("hex");
        assert.equal(item.sha256, expected, `dispute ${dispute.id} evidence ${item.index}`);
        assert.equal(item.bytes_len, Buffer.byteLength(item.text, "utf8"));
      }
    }
  });

  test("sample majorities match the recorded outcomes", () => {
    for (const dispute of SAMPLE.disputes) {
      if (dispute.reveals.length === 0) continue;
      const votes = dispute.reveals.map((r) => r.vote);
      const outcome = majorityOutcome(votes, dispute.panel.length);
      assert.equal(outcome.vote, dispute.outcome === "pay" ? VOTE_PAY : VOTE_REFUND);
    }
  });

  test("sample amounts match the settlement rules and conserve exactly", () => {
    for (const dispute of SAMPLE.disputes) {
      if (dispute.reveals.length === 0) continue;
      const votes = dispute.reveals.map((r) => r.vote);
      const outcome = majorityOutcome(votes, dispute.panel.length);
      const amounts = settleAmounts(dispute.principal, outcome.vote, outcome.count);
      assert.equal(dispute.payeeAmount, amounts.payeeAmount, `dispute ${dispute.id} payee`);
      assert.equal(dispute.payerAmount, amounts.payerAmountNet, `dispute ${dispute.id} payer`);
      assert.equal(
        dispute.payerAmountGross,
        amounts.payerAmount,
        `dispute ${dispute.id} payer gross`,
      );
      assert.equal(dispute.fees, amounts.fees, `dispute ${dispute.id} fees`);
      assert.equal(
        dispute.payeeAmount + dispute.payerAmount + dispute.fees,
        dispute.principal,
        `dispute ${dispute.id} conservation`,
      );
    }
  });
});

describe("hexToBytes", () => {
  test("bad hex is rejected instead of becoming null bytes", () => {
    assert.throws(() => hexToBytes("zz"), /only hex characters/);
    assert.throws(() => hexToBytes("0x12g4"), /only hex characters/);
    assert.throws(() => hexToBytes("abc"), /even length/);
    assert.throws(() => hexToBytes(123), /must be a string/);
    assert.deepEqual([...hexToBytes("0x00ff")], [0, 255]);
    assert.deepEqual([...hexToBytes("aAbB")], [0xaa, 0xbb]);
  });
});

describe("u64ToLeBytes", () => {
  test("negatives, fractions, and out-of-range values are rejected, not wrapped", () => {
    assert.throws(() => u64ToLeBytes(-1), /out of range/);
    assert.throws(() => u64ToLeBytes(1.5), /whole number/);
    assert.throws(() => u64ToLeBytes(2n ** 64n), /out of range/);
    assert.throws(() => u64ToLeBytes("12"), /whole number/);
    assert.deepEqual([...u64ToLeBytes(0n)], [0, 0, 0, 0, 0, 0, 0, 0]);
    assert.deepEqual(
      [...u64ToLeBytes(0xffffffffffffffffn)],
      [255, 255, 255, 255, 255, 255, 255, 255],
    );
    assert.deepEqual([...u64ToLeBytes(258)], [2, 1, 0, 0, 0, 0, 0, 0]);
  });

  test("a number above the safe integer range is refused, not rounded", () => {
    assert.throws(() => u64ToLeBytes(2 ** 53), /safe integer range/);
    assert.throws(() => u64ToLeBytes(Number.MAX_SAFE_INTEGER + 1), /safe integer range/);
    assert.deepEqual([...u64ToLeBytes(Number.MAX_SAFE_INTEGER)], [255, 255, 255, 255, 255, 255, 31, 0]);
  });
});

describe("domain separation", () => {
  test("the domain is a separate hash input, and the two draws in this test differ", () => {
    assert.equal(MAX_RESCHEDULES, 3);
    const pool = ["a", "b", "c", "d", "e", "f", "g"];
    const hash = sha256Hex("domain-separation-check");
    // The old arithmetic collided here: round 6 with domain 0
    // contributed the same as round 0 with domain 1. The hash
    // construction derives the two draws independently; this test
    // shows only that these two draws differ, not that draws can
    // never match.
    const first = derivePanel(pool, hash, 0, 6, DRAW_DOMAIN_FIRST, 3);
    const appeal = derivePanel(pool, hash, 0, 0, DRAW_DOMAIN_APPEAL, 3);
    assert.notDeepEqual(first, appeal);
    assert.notDeepEqual(
      derivePanel(pool, hash, 0, 0, DRAW_DOMAIN_FIRST, 3),
      derivePanel(pool, hash, 0, 0, DRAW_DOMAIN_APPEAL, 3),
    );
  });
});

describe("BigInt settlement amounts", () => {
  test("a principal above the safe integer range keeps every micro-Tari", () => {
    const principal = 9007199254740993n;
    assert.deepEqual(settleAmounts(principal, VOTE_PAY, 2), {
      fees: 10000n,
      payeeAmount: principal - 10000n,
      payerAmount: 10000n,
      payerAmountNet: 0n,
    });
    const split = settleAmounts(principal, VOTE_SPLIT, 2);
    assert.equal(split.payeeAmount + split.payerAmountNet + split.fees, principal);
    assert.equal(split.payeeAmount - split.payerAmountNet, 1n);
    assert.throws(() => settleAmounts(9007199254740993, VOTE_PAY, 2), /whole number/);
  });

  test("splitLargestRemainder in BigInt conserves exactly", () => {
    const parts = splitLargestRemainder(10000000000000000003n, [3n, 2n, 1n]);
    assert.equal(parts.reduce((a, b) => a + b, 0n), 10000000000000000003n);
    assert.deepEqual(splitLargestRemainder(10n, [1, 1, 1]), [4n, 3n, 3n]);
  });

  test("a negative BigInt principal or weight is refused, not computed", () => {
    assert.throws(() => settleAmounts(-500000n, VOTE_PAY, 2), /must not be negative/);
    assert.throws(() => settleAmounts(500000n, VOTE_PAY, -2n), /must not be negative/);
    assert.throws(() => splitLargestRemainder(10n, [2n, -1n]), /must not be negative/);
    assert.throws(() => splitLargestRemainder(-10n, [1n, 1n]), /must not be negative/);
  });

  test("a negative or fractional Number total or weight is refused, not computed", () => {
    assert.throws(() => splitLargestRemainder(-10, [1, 1]), /non-negative whole number/);
    assert.throws(() => splitLargestRemainder(10, [-1, 2]), /non-negative whole numbers/);
    assert.throws(() => splitLargestRemainder(10, [0.5, 0.5]), /non-negative whole numbers/);
    assert.throws(() => splitLargestRemainder(10.5, [1, 1]), /non-negative whole number/);
    assert.throws(() => splitLargestRemainder(2 ** 53, [1, 1]), /safe integer range/);
    assert.deepEqual(splitLargestRemainder(10, [1, 1]), [5, 5]);
  });
});
