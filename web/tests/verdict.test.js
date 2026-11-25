// Tests for web/src/verdict.js and the generated sample data.
// Run from web/: node --test tests/
// These are logic tests for the proof page. They are not parity tests: the
// full simulation parity suite waits for Phase 3, because the simulation
// must mirror the template rule for rule and Phase 3 is rewriting the
// settlement matrix.

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  PANEL_SIZE,
  JUROR_FEE,
  BOND_FLOOR,
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
  splitLargestRemainder,
  callerRuleCheck,
} from "../src/verdict.js";
import { SAMPLE } from "../src/sample-data.js";

describe("template constants", () => {
  test("panel size, juror fee, and bond floor match the template", () => {
    assert.equal(PANEL_SIZE, 3);
    assert.equal(JUROR_FEE, 5000);
    assert.equal(BOND_FLOOR, 30000);
    assert.equal(BOND_FLOOR, 2 * JUROR_FEE * PANEL_SIZE);
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

  test("small pool returns fewer jurors, empty pool returns none", () => {
    assert.equal(derivePanel(pool.slice(0, 2), drawHash, 0, 0).length, 2);
    assert.deepEqual(derivePanel([], drawHash, 0, 0), []);
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

  test("majority refuse is a valid outcome", () => {
    assert.deepEqual(majorityOutcome([VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]), {
      vote: VOTE_REFUSE,
      count: 2,
    });
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

  test("zero weights are rejected", () => {
    assert.throws(() => splitLargestRemainder(100, [0, 0]), /weights must sum above zero/);
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
      const outcome = majorityOutcome(votes);
      assert.equal(outcome.vote, dispute.outcome === "pay" ? VOTE_PAY : VOTE_REFUND);
    }
  });
});
