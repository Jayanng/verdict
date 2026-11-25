// Verdict web app: hash router, pages, and the proof-page tamper checks.
// Static site, no build step. All hashing runs in the browser with the pure
// SHA-256 in src/verdict.js. All data shown is sample data, labelled on the
// page, until the live path via Sapient is wired.

import { SAMPLE } from "./sample-data.js";
import {
  PANEL_SIZE,
  JUROR_FEE,
  BOND_FLOOR,
  EVIDENCE_LIMIT,
  VOTE_LABELS,
  derivePanel,
  majorityOutcome,
  callerRuleCheck,
  sha256Hex,
  voteCommitmentBytes,
} from "./verdict.js";

const REPO = "https://github.com/Jayanng/verdict";
const COURT_COMPONENT = "sample-component-court";

const page = document.getElementById("page");
const HOME_HTML = page.innerHTML;

const TICKER_ITEMS = [
  "court never touches the money",
  "escrow executes the ruling",
  "caller rule accepts exactly one component",
  "draw recomputable from published inputs",
  "ruling, appeal, or bounded fallback",
  "loser pays from principal",
];

const FAQ_ITEMS = [
  {
    q: "Who decides a dispute?",
    a: "Three bonded strangers, drawn from a pool of jurors who posted a bond. The majority rules: pay, refund, or split. A tie goes to appeal with seven jurors, funded by the appellant.",
  },
  {
    q: "What does the court hold?",
    a: "Nothing. The court holds no money and no vault. It issues a ruling, and the escrow's own ownerless code moves its own vault. The court only ever receives the buckets the escrow returns.",
  },
  {
    q: "What does a juror post, and what does a juror earn?",
    a: "A juror posts a bond of at least 30,000 micro-Tari, which is twice the largest juror fee times the panel size. A juror earns 5,000 micro-Tari per dispute. A juror who votes against the majority forfeits bond to the majority jurors.",
  },
  {
    q: "What if a juror disappears?",
    a: "A juror who commits but never reveals is replaced by a new draw. The replacement draw commits a new future epoch at the moment the failure is recorded. If quorum still fails after one replacement draw, the bounded no-ruling fallback closes the dispute and unfreezes the escrow.",
  },
  {
    q: "Can I check a ruling myself?",
    a: "Yes. Every dispute page is a proof page. It shows the draw inputs with a recompute button, the evidence hashes with a live check, and the caller rule. Change one draw input and watch the panel change. Change one byte of evidence and watch the hash check fail.",
  },
  {
    q: "Is this live on chain yet?",
    a: "No. The live path via Sapient is not wired yet, so this app shows sample inputs, clearly labelled as samples, and the proof checks run in your browser against those inputs. The engine tests in the repo are the live verification so far: 102 tests, run with cargo test.",
  },
];

function esc(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function fmt(n) {
  return Number(n).toLocaleString("en-US");
}

function shortHex(hex) {
  if (!hex) return "";
  return hex.length > 20 ? `${hex.slice(0, 10)}...${hex.slice(-6)}` : hex;
}

function sampleNote(text) {
  return `<div class="sample-note"><b>Sample data.</b> ${text}</div>`;
}

function statusPill(status) {
  if (status === "settled") return `<span class="rui-pill green">Settled</span>`;
  return `<span class="rui-pill amber">${esc(status)}</span>`;
}

// ----- home widgets -----

function initHome() {
  const track = document.getElementById("tickerTrack");
  if (track) {
    const items = TICKER_ITEMS.map((t) => `<span><b>&#10003;</b>${esc(t)}</span>`).join("");
    track.innerHTML = items + items;
  }
  const faqCard = document.getElementById("faqCard");
  if (faqCard) {
    faqCard.innerHTML = FAQ_ITEMS.map(
      (item) => `<div class="rui-accordion-item">
        <button class="rui-accordion-q" type="button">${esc(item.q)}<span class="pm">+</span></button>
        <div class="rui-accordion-a"><p>${esc(item.a)}</p></div>
      </div>`,
    ).join("");
    faqCard.querySelectorAll(".rui-accordion-q").forEach((button) => {
      button.addEventListener("click", () => {
        button.parentElement.classList.toggle("open");
      });
    });
  }
}

function scrollToSection(id) {
  const el = document.getElementById(id);
  if (el) el.scrollIntoView({ behavior: "smooth" });
}

// ----- disputes list -----

function renderApp() {
  const rows = SAMPLE.disputes.map((d) => {
    const outcome = d.outcome ? d.outcome : "open";
    return `<div class="dispute-row">
      <div class="body">
        <h3><a href="#/dispute/${d.id}">Dispute #${d.id}: ${esc(d.title)}</a></h3>
        <p>${esc(d.escrowType)} between ${esc(d.payer)} and ${esc(d.payee)}.</p>
        <div class="meta">
          <span>Principal <b>${fmt(d.principal)} &micro;T</b></span>
          <span>Status <b>${esc(d.status)}</b></span>
          <span>Outcome <b>${esc(outcome)}</b></span>
        </div>
      </div>
      ${statusPill(d.status)}
    </div>`;
  }).join("");
  page.innerHTML = `
    <section class="section tight">
      <div class="wrap">
        <div class="sec-head">
          <h2>Disputes.</h2>
          <p class="sec-note">Every dispute is a proof page: draw inputs, panel, commits, reveals, ruling, and the caller rule. Click one to check it.</p>
        </div>
        ${sampleNote("These three disputes are samples, generated from the template rules so the proof checks pass on them. No money moved, no key is a real person, and no hash is on chain.")}
        <div class="rui-card mt-24" style="padding: 4px 26px;">${rows}</div>
        <div class="form-actions">
          <a class="rui-btn" href="#/open">Open a dispute</a>
          <a class="rui-btn rui-btn-ghost" href="#/juror">Become a juror</a>
        </div>
      </div>
    </section>`;
  window.scrollTo(0, 0);
}

// ----- dispute detail and proof page -----

function disputeById(id) {
  return SAMPLE.disputes.find((d) => d.id === id);
}

function renderDispute(id) {
  const d = disputeById(id);
  if (!d) {
    page.innerHTML = `<section class="section tight"><div class="wrap">
      <h2>No such dispute.</h2>
      <p class="lede">Dispute #${esc(id)} is not in the sample set.</p>
      <div class="form-actions"><a class="rui-btn" href="#/app">Back to disputes</a></div>
    </div></section>`;
    window.scrollTo(0, 0);
    return;
  }
  const majority = d.reveals.length ? majorityOutcome(d.reveals.map((r) => r.vote)) : null;
  const panelRows = d.panel.map((p) => `<tr>
      <td>${esc(p.label)}</td>
      <td class="mono">${shortHex(p.key)}</td>
    </tr>`).join("");
  const voteRows = d.commits.map((c) => `<tr>
      <td>${esc(c.jurorLabel)}</td>
      <td>${esc(VOTE_LABELS[c.vote])}</td>
      <td class="mono">${shortHex(c.commitment)}</td>
      <td><button class="rui-btn rui-btn-sm rui-btn-ghost" type="button" data-verify-commit='${esc(JSON.stringify(c))}' data-dispute="${d.id}">Verify</button></td>
      <td class="verify-result"></td>
    </tr>`).join("") || `<tr><td colspan="5">No votes yet. The panel is drawn and voting is open.</td></tr>`;
  const evidenceRows = d.evidence.map((item) => `<div class="rui-check" data-evidence>
      <span class="mark">&#10003;</span>
      <div class="body">
        <div class="t">${esc(item.side)} evidence #${item.index}, ${fmt(item.bytes_len)} bytes</div>
        <div class="d">${esc(item.text)}</div>
        <div class="mono">stored hash ${esc(item.sha256)}</div>
        <div class="mono">location ${esc(item.uri)}</div>
        <div class="form-actions" style="margin-top: 10px;">
          <button class="rui-btn rui-btn-sm" type="button" data-check-evidence='${esc(JSON.stringify(item))}'>Check hash now</button>
        </div>
        <div class="result-box hidden"></div>
      </div>
    </div>`).join("");
  const ruling = d.status === "settled" ? `
    <div class="rui-card mt-24">
      <h3>Ruling: ${esc(d.outcome)}</h3>
      <p class="lede" style="margin-top: 8px;">${esc(d.ruling)}</p>
      <div class="fact-grid mt-24">
        <div class="fact"><div class="k">Payee amount</div><div class="v">${fmt(d.payeeAmount)} &micro;T</div></div>
        <div class="fact"><div class="k">Remainder to payer</div><div class="v">${fmt(d.principal - d.payeeAmount)} &micro;T</div></div>
        <div class="fact"><div class="k">Majority</div><div class="v">${majority ? `${majority.count} of ${d.reveals.length} jurors` : "none"}</div></div>
        <div class="fact"><div class="k">Settlement record</div><div class="v mono">${esc(d.settlementTx)}</div></div>
      </div>
      <p class="sec-note" style="margin-top: 16px;">Routing as it stands in the templates at this commit: the court calls execute_ruling with the payee amount, the payee bucket goes to the payee account, and the remainder bucket goes straight to the payer account. The settlement record above is a sample label, not a transaction on chain. How juror fees are funded from the principal is set by the settlement matrix, which is being written now.</p>
    </div>` : `
    <div class="rui-card mt-24">
      <h3>Ruling: not yet made</h3>
      <p class="lede" style="margin-top: 8px;">The panel is drawn and voting is open. The ruling appears here once a majority reveals.</p>
    </div>`;

  page.innerHTML = `
    <section class="section tight">
      <div class="wrap">
        <div class="sec-head">
          <h2>Dispute #${d.id}: ${esc(d.title)}</h2>
          <p class="sec-note">${esc(d.escrowType)} between ${esc(d.payer)} and ${esc(d.payee)}. Principal ${fmt(d.principal)} micro-Tari.</p>
        </div>
        ${sampleNote("This dispute is a sample. Its panel really is derived from its draw hash, and every commitment and evidence hash really is the SHA-256 of the bytes it claims to cover. Check any of them below.")}
        <div class="form-actions" style="margin-bottom: 24px;">
          <a class="rui-btn rui-btn-ghost" href="#/app">&larr; All disputes</a>
          ${statusPill(d.status)}
        </div>
        <div class="rui-card">
          <div class="fact-grid">
            <div class="fact"><div class="k">Escrow</div><div class="v mono">${esc(d.escrow)}</div></div>
            <div class="fact"><div class="k">Open epoch</div><div class="v">${d.openEpoch}</div></div>
            <div class="fact"><div class="k">Draw epoch</div><div class="v">${d.drawEpoch}</div></div>
            <div class="fact"><div class="k">Draw round</div><div class="v">${d.round}</div></div>
            <div class="fact"><div class="k">Payer key</div><div class="v mono">${shortHex(d.payerKey)}</div></div>
            <div class="fact"><div class="k">Payee key</div><div class="v mono">${shortHex(d.payeeKey)}</div></div>
            <div class="fact"><div class="k">Milestones</div><div class="v">${d.milestones.map((m) => fmt(m)).join(" + ")} &micro;T</div></div>
            <div class="fact"><div class="k">Court</div><div class="v mono">${esc(COURT_COMPONENT)}</div></div>
          </div>
        </div>

        <div class="rui-card mt-24">
          <h3>Panel, ${d.panel.length} jurors drawn</h3>
          <table class="rui-table mt-16"><thead><tr><th>Juror</th><th>Key</th></tr></thead><tbody>${panelRows}</tbody></table>
        </div>

        <div class="rui-card mt-24">
          <h3>Commits and reveals</h3>
          <p class="sec-note">Each commitment is SHA-256 of (vote, salt, dispute id, juror). Press Verify to recompute one in this browser and compare it to the stored commitment.</p>
          <table class="rui-table mt-16"><thead><tr><th>Juror</th><th>Vote</th><th>Commitment</th><th></th><th></th></tr></thead><tbody>${voteRows}</tbody></table>
        </div>

        ${ruling}

        <div class="rui-card mt-24">
          <h3>Evidence, hash checked at display time</h3>
          <p class="sec-note">Each item shows the hash stored when it was posted. Press Check hash now to recompute it from the bytes.</p>
          ${evidenceRows}
        </div>

        <div class="rui-card mt-24 lab">
          <h3>Break it yourself: three live checks</h3>
          <p class="lab-note">Each check runs in your browser against the published inputs. Nothing here is a canned animation, and nothing here touches a chain.</p>

          <div class="lab-grid">
            <div>
              <h3>1. Change a draw input, watch the panel change</h3>
              <p class="lab-note">The panel is derived from the draw hash, the dispute id, and the round, over the frozen pool of ${SAMPLE.pool.length} jurors.</p>
              <div class="rui-field"><label for="drawHash">Draw hash, future epoch ${d.drawEpoch}</label>
                <input class="rui-input mono" id="drawHash" value="${esc(d.drawHash)}" spellcheck="false"></div>
              <div class="rui-field mt-16"><label for="drawDisputeId">Dispute id</label>
                <input class="rui-input" id="drawDisputeId" type="number" value="${d.id}"></div>
              <div class="rui-field mt-16"><label for="drawRound">Round</label>
                <input class="rui-input" id="drawRound" type="number" value="${d.round}"></div>
              <div class="form-actions">
                <button class="rui-btn" type="button" id="recomputePanel">Recompute panel</button>
                <button class="rui-btn rui-btn-ghost" type="button" id="flipDrawByte">Change one byte of the hash</button>
              </div>
              <div class="result-box" id="panelResult">Panel not recomputed yet. The recorded panel is ${d.panel.map((p) => esc(p.label)).join(", ")}.</div>
            </div>
            <div>
              <h3>2. Change one byte of evidence, watch the hash fail</h3>
              <p class="lab-note">The stored hash below covers the exact bytes shown. Edit one character and the check fails.</p>
              <div class="rui-field"><label for="tamperText">Evidence bytes</label>
                <textarea class="rui-textarea" id="tamperText" rows="4">${esc(d.evidence[0].text)}</textarea></div>
              <div class="rui-field mt-16"><label>Stored hash</label>
                <div class="mono" style="font-size: 12px; word-break: break-all;">${esc(d.evidence[0].sha256)}</div></div>
              <div class="result-box" id="tamperResult">Hash not checked yet.</div>
              <div class="form-actions">
                <button class="rui-btn rui-btn-ghost" type="button" id="restoreEvidence">Restore original bytes</button>
              </div>
            </div>
          </div>

          <div class="mt-24">
            <h3>3. Call execute_ruling as a stranger, watch the caller rule refuse</h3>
            <p class="lab-note">The escrow's settlement method accepts calls from exactly one component, the court named as arbitrator at creation. This check runs that published rule in your browser against the published inputs. It is not a chain call.</p>
            <div class="form-actions">
              <button class="rui-btn" type="button" id="callAsCourt">Call as the court</button>
              <button class="rui-btn rui-btn-ghost" type="button" id="callAsStranger">Call as a stranger</button>
            </div>
            <div class="result-box" id="callerResult">No call made yet.</div>
          </div>
        </div>
      </div>
    </section>`;
  window.scrollTo(0, 0);
  initDisputePage(d);
}

function initDisputePage(d) {
  page.querySelectorAll("[data-verify-commit]").forEach((button) => {
    button.addEventListener("click", () => {
      const commit = JSON.parse(button.dataset.verifyCommit);
      const disputeId = Number(button.dataset.dispute);
      const expected = sha256Hex(
        voteCommitmentBytes(commit.vote, commit.salt, disputeId, commit.juror),
      );
      const cell = button.closest("tr").querySelector(".verify-result");
      if (expected === commit.commitment) {
        cell.innerHTML = `<span class="rui-pill green">Match</span>`;
      } else {
        cell.innerHTML = `<span class="rui-pill red">Mismatch</span>`;
      }
    });
  });

  page.querySelectorAll("[data-check-evidence]").forEach((button) => {
    button.addEventListener("click", () => {
      const item = JSON.parse(button.dataset.checkEvidence);
      const computed = sha256Hex(item.text);
      const box = button.closest(".rui-check").querySelector(".result-box");
      box.classList.remove("hidden");
      if (computed === item.sha256) {
        box.classList.add("pass");
        box.classList.remove("fail");
        box.innerHTML = `<b>Hash matches.</b> Recomputed ${computed.slice(0, 16)}..., equal to the stored hash.`;
      } else {
        box.classList.add("fail");
        box.classList.remove("pass");
        box.innerHTML = `<b>Hash does not match.</b> Recomputed ${computed.slice(0, 16)}..., stored ${item.sha256.slice(0, 16)}...`;
      }
    });
  });

  const drawHashInput = document.getElementById("drawHash");
  const drawDisputeInput = document.getElementById("drawDisputeId");
  const drawRoundInput = document.getElementById("drawRound");
  const panelResult = document.getElementById("panelResult");
  const recordedLabels = d.panel.map((p) => p.label).join(", ");
  document.getElementById("recomputePanel").addEventListener("click", () => {
    try {
      const indexes = derivePanel(
        SAMPLE.pool,
        drawHashInput.value.trim(),
        Number(drawDisputeInput.value),
        Number(drawRoundInput.value),
      );
      const labels = indexes.map((i) => SAMPLE.jurors[i].label).join(", ");
      const changed = labels !== recordedLabels;
      panelResult.classList.toggle("pass", !changed);
      panelResult.classList.toggle("fail", changed);
      panelResult.innerHTML = changed
        ? `<b>Panel changed.</b> Recomputed panel: ${esc(labels)}. Recorded panel: ${esc(recordedLabels)}.`
        : `<b>Panel matches.</b> Recomputed panel: ${esc(labels)}, equal to the recorded panel.`;
    } catch (error) {
      panelResult.classList.add("fail");
      panelResult.innerHTML = `<b>Cannot recompute.</b> ${esc(error.message)}`;
    }
  });
  document.getElementById("flipDrawByte").addEventListener("click", () => {
    const hash = drawHashInput.value.trim();
    const first = hash[0] === "0" ? "1" : "0";
    drawHashInput.value = first + hash.slice(1);
    document.getElementById("recomputePanel").click();
  });

  const tamperText = document.getElementById("tamperText");
  const tamperResult = document.getElementById("tamperResult");
  const storedHash = d.evidence[0].sha256;
  const checkTamper = () => {
    const computed = sha256Hex(tamperText.value);
    if (computed === storedHash) {
      tamperResult.classList.add("pass");
      tamperResult.classList.remove("fail");
      tamperResult.innerHTML = `<b>Hash matches.</b> The bytes are exactly what was posted.`;
    } else {
      tamperResult.classList.add("fail");
      tamperResult.classList.remove("pass");
      tamperResult.innerHTML = `<b>Hash fails.</b> Computed ${computed.slice(0, 16)}..., stored ${storedHash.slice(0, 16)}.... One changed byte breaks the check.`;
    }
  };
  tamperText.addEventListener("input", checkTamper);
  document.getElementById("restoreEvidence").addEventListener("click", () => {
    tamperText.value = d.evidence[0].text;
    checkTamper();
  });
  checkTamper();

  const callerResult = document.getElementById("callerResult");
  const showCaller = (caller) => {
    const check = callerRuleCheck(caller, COURT_COMPONENT);
    callerResult.classList.toggle("pass", check.allowed);
    callerResult.classList.toggle("fail", !check.allowed);
    callerResult.innerHTML = check.allowed
      ? `<b>Call accepted.</b> ${esc(check.reason)}`
      : `<b>Call refused.</b> ${esc(check.reason)}`;
  };
  document.getElementById("callAsCourt").addEventListener("click", () => showCaller(COURT_COMPONENT));
  document.getElementById("callAsStranger").addEventListener("click", () => showCaller("sample-component-stranger"));
}

// ----- open a dispute, guided four-step form -----

const draft = {
  step: 1,
  escrowType: "Milestone escrow",
  principal: 500000,
  milestone1: 200000,
  milestone2: 300000,
  payerName: "",
  payeeName: "",
  payerKey: "",
  payeeKey: "",
  evidenceSide: "payer",
  evidence: [],
};

function renderOpen() {
  page.innerHTML = `
    <section class="section tight">
      <div class="wrap">
        <div class="sec-head">
          <h2>Open a dispute.</h2>
          <p class="sec-note">Four steps. This form builds a local draft in your browser. Submitting a dispute on chain needs the live path, which is not wired yet, so nothing here moves money.</p>
        </div>
        <div class="rui-steps" id="openSteps"></div>
        <div class="rui-card" id="openBody"></div>
      </div>
    </section>`;
  window.scrollTo(0, 0);
  renderOpenStep();
}

function openStepsHtml() {
  const labels = ["1. Deal", "2. Parties", "3. Evidence", "4. Review"];
  return labels.map((label, i) => {
    const n = i + 1;
    const cls = n === draft.step ? "active" : n < draft.step ? "done" : "";
    return `<div class="rui-step ${cls}">${esc(label)}</div>`;
  }).join("");
}

function renderOpenStep() {
  document.getElementById("openSteps").innerHTML = openStepsHtml();
  const body = document.getElementById("openBody");
  if (draft.step === 1) {
    body.innerHTML = `
      <h3>The deal</h3>
      <p class="sec-note">What kind of escrow holds the money, and how much is in it.</p>
      <div class="rui-field"><label for="fEscrowType">Escrow type</label>
        <select class="rui-select" id="fEscrowType">
          <option ${draft.escrowType === "Milestone escrow" ? "selected" : ""}>Milestone escrow</option>
          <option ${draft.escrowType === "Payment escrow" ? "selected" : ""}>Payment escrow</option>
        </select></div>
      <div class="rui-field mt-16"><label for="fPrincipal">Principal, micro-Tari</label>
        <input class="rui-input" id="fPrincipal" type="number" min="1" value="${draft.principal}"></div>
      <div id="milestoneFields" class="${draft.escrowType === "Milestone escrow" ? "" : "hidden"}">
        <div class="rui-field mt-16"><label for="fM1">Milestone 1 amount, micro-Tari</label>
          <input class="rui-input" id="fM1" type="number" min="1" value="${draft.milestone1}"></div>
        <div class="rui-field mt-16"><label for="fM2">Milestone 2 amount, micro-Tari</label>
          <input class="rui-input" id="fM2" type="number" min="1" value="${draft.milestone2}"></div>
        <p class="sec-note">Milestone amounts must add up to the principal.</p>
      </div>
      <div class="result-box hidden" id="stepError"></div>
      <div class="form-actions"><span class="spacer"></span>
        <button class="rui-btn" type="button" id="openNext">Next: parties</button></div>`;
    document.getElementById("fEscrowType").addEventListener("change", (e) => {
      draft.escrowType = e.target.value;
      renderOpenStep();
    });
    document.getElementById("openNext").addEventListener("click", () => {
      draft.principal = Number(document.getElementById("fPrincipal").value);
      const error = document.getElementById("stepError");
      if (!draft.principal || draft.principal <= 0) {
        error.classList.remove("hidden");
        error.classList.add("fail");
        error.textContent = "The principal must be a positive amount.";
        return;
      }
      if (draft.escrowType === "Milestone escrow") {
        draft.milestone1 = Number(document.getElementById("fM1").value);
        draft.milestone2 = Number(document.getElementById("fM2").value);
        if (draft.milestone1 + draft.milestone2 !== draft.principal) {
          error.classList.remove("hidden");
          error.classList.add("fail");
          error.textContent = "Milestone amounts must add up to the principal.";
          return;
        }
      }
      draft.step = 2;
      renderOpenStep();
    });
  }
  if (draft.step === 2) {
    body.innerHTML = `
      <h3>The parties</h3>
      <p class="sec-note">Who paid, and who was meant to be paid. Keys are optional here; a blank key becomes a sample key in the draft.</p>
      <div class="rui-field"><label for="fPayerName">Payer name</label>
        <input class="rui-input" id="fPayerName" value="${esc(draft.payerName)}" placeholder="Ada, client"></div>
      <div class="rui-field mt-16"><label for="fPayeeName">Payee name</label>
        <input class="rui-input" id="fPayeeName" value="${esc(draft.payeeName)}" placeholder="Bola, freelancer"></div>
      <div class="rui-field mt-16"><label for="fPayerKey">Payer public key <span class="hint">(optional)</span></label>
        <input class="rui-input mono" id="fPayerKey" value="${esc(draft.payerKey)}" spellcheck="false"></div>
      <div class="rui-field mt-16"><label for="fPayeeKey">Payee public key <span class="hint">(optional)</span></label>
        <input class="rui-input mono" id="fPayeeKey" value="${esc(draft.payeeKey)}" spellcheck="false"></div>
      <div class="result-box hidden" id="stepError"></div>
      <div class="form-actions">
        <button class="rui-btn rui-btn-ghost" type="button" id="openBack">Back</button><span class="spacer"></span>
        <button class="rui-btn" type="button" id="openNext">Next: evidence</button></div>`;
    document.getElementById("openBack").addEventListener("click", () => { draft.step = 1; renderOpenStep(); });
    document.getElementById("openNext").addEventListener("click", () => {
      draft.payerName = document.getElementById("fPayerName").value.trim();
      draft.payeeName = document.getElementById("fPayeeName").value.trim();
      draft.payerKey = document.getElementById("fPayerKey").value.trim();
      draft.payeeKey = document.getElementById("fPayeeKey").value.trim();
      const error = document.getElementById("stepError");
      if (!draft.payerName || !draft.payeeName) {
        error.classList.remove("hidden");
        error.classList.add("fail");
        error.textContent = "Both party names are needed.";
        return;
      }
      draft.step = 3;
      renderOpenStep();
    });
  }
  if (draft.step === 3) {
    const items = draft.evidence.map((item, i) => `<div class="rui-check">
        <span class="mark">&#10003;</span>
        <div class="body">
          <div class="t">${esc(item.side)} evidence #${item.index}</div>
          <div class="d">${esc(item.text)}</div>
          <div class="mono">SHA-256 ${esc(item.sha256)}</div>
        </div>
        <button class="rui-btn rui-btn-sm rui-btn-ghost" type="button" data-remove-evidence="${i}">Remove</button>
      </div>`).join("");
    const sideCount = draft.evidence.filter((item) => item.side === draft.evidenceSide).length;
    body.innerHTML = `
      <h3>Evidence</h3>
      <p class="sec-note">Up to ${EVIDENCE_LIMIT} items per side. Each item is hashed in your browser the moment you add it, and the hash is what the court records.</p>
      <div class="rui-field"><label for="fEvidenceSide">Side</label>
        <select class="rui-select" id="fEvidenceSide">
          <option value="payer" ${draft.evidenceSide === "payer" ? "selected" : ""}>Payer</option>
          <option value="payee" ${draft.evidenceSide === "payee" ? "selected" : ""}>Payee</option>
        </select></div>
      <div class="rui-field mt-16"><label for="fEvidenceText">Evidence text</label>
        <textarea class="rui-textarea" id="fEvidenceText" rows="3" placeholder="Paste the delivery note, message, or receipt text."></textarea></div>
      <div class="form-actions">
        <button class="rui-btn" type="button" id="addEvidence" ${sideCount >= EVIDENCE_LIMIT ? "disabled" : ""}>Add evidence item</button>
        <span class="sec-note">${sideCount} of ${EVIDENCE_LIMIT} items for the ${esc(draft.evidenceSide)} side.</span>
      </div>
      <div class="mt-24">${items || `<p class="sec-note">No evidence added yet. Evidence is optional in the draft, and the court records at most ${EVIDENCE_LIMIT} items per side.</p>`}</div>
      <div class="form-actions">
        <button class="rui-btn rui-btn-ghost" type="button" id="openBack">Back</button><span class="spacer"></span>
        <button class="rui-btn" type="button" id="openNext">Next: review</button></div>`;
    document.getElementById("fEvidenceSide").addEventListener("change", (e) => {
      draft.evidenceSide = e.target.value;
      renderOpenStep();
    });
    document.getElementById("addEvidence").addEventListener("click", () => {
      const text = document.getElementById("fEvidenceText").value;
      if (!text.trim()) return;
      const index = draft.evidence.filter((item) => item.side === draft.evidenceSide).length;
      draft.evidence.push({
        side: draft.evidenceSide,
        index,
        text,
        bytes_len: new TextEncoder().encode(text).length,
        sha256: sha256Hex(text),
      });
      renderOpenStep();
    });
    body.querySelectorAll("[data-remove-evidence]").forEach((button) => {
      button.addEventListener("click", () => {
        draft.evidence.splice(Number(button.dataset.removeEvidence), 1);
        for (const side of ["payer", "payee"]) {
          draft.evidence
            .filter((item) => item.side === side)
            .forEach((item, i) => { item.index = i; });
        }
        renderOpenStep();
      });
    });
    document.getElementById("openBack").addEventListener("click", () => { draft.step = 2; renderOpenStep(); });
    document.getElementById("openNext").addEventListener("click", () => { draft.step = 4; renderOpenStep(); });
  }
  if (draft.step === 4) {
    const milestones = draft.escrowType === "Milestone escrow"
      ? `${fmt(draft.milestone1)} + ${fmt(draft.milestone2)}`
      : `${fmt(draft.principal)} (single payment)`;
    const evidenceRows = draft.evidence.map((item) => `<tr>
        <td>${esc(item.side)} #${item.index}</td>
        <td>${esc(item.text.slice(0, 60))}${item.text.length > 60 ? "..." : ""}</td>
        <td class="mono">${shortHex(item.sha256)}</td>
      </tr>`).join("") || `<tr><td colspan="3">No evidence added.</td></tr>`;
    const draftJson = JSON.stringify({
      escrowType: draft.escrowType,
      principal: draft.principal,
      payer: draft.payerName,
      payee: draft.payeeName,
      evidence: draft.evidence,
    }, null, 2);
    body.innerHTML = `
      <h3>Review the draft</h3>
      <p class="sec-note">Check everything before saving. This draft lives in your browser session only.</p>
      <table class="rui-table mt-16"><tbody>
        <tr><td><b>Escrow</b></td><td>${esc(draft.escrowType)}</td></tr>
        <tr><td><b>Principal</b></td><td>${fmt(draft.principal)} &micro;T</td></tr>
        <tr><td><b>Milestones</b></td><td>${milestones} &micro;T</td></tr>
        <tr><td><b>Payer</b></td><td>${esc(draft.payerName)}</td></tr>
        <tr><td><b>Payee</b></td><td>${esc(draft.payeeName)}</td></tr>
      </tbody></table>
      <h3 class="mt-24">Evidence, ${draft.evidence.length} item(s)</h3>
      <table class="rui-table mt-16"><thead><tr><th>Item</th><th>Text</th><th>SHA-256</th></tr></thead><tbody>${evidenceRows}</tbody></table>
      <div class="result-box hidden" id="draftSaved"></div>
      <div class="form-actions">
        <button class="rui-btn rui-btn-ghost" type="button" id="openBack">Back</button><span class="spacer"></span>
        <button class="rui-btn" type="button" id="saveDraft">Save draft</button>
        <button class="rui-btn rui-btn-ghost" type="button" id="copyDraft">Copy draft</button>
      </div>
      <p class="sec-note" style="margin-top: 16px;">Saving keeps the draft in this browser session only. Opening a real dispute on chain needs the live path via Sapient, which is not wired yet. When it is, this same draft shape is what gets submitted.</p>
      <textarea class="rui-textarea hidden" id="draftJson" readonly rows="8">${esc(draftJson)}</textarea>`;
    document.getElementById("openBack").addEventListener("click", () => { draft.step = 3; renderOpenStep(); });
    document.getElementById("saveDraft").addEventListener("click", () => {
      const box = document.getElementById("draftSaved");
      box.classList.remove("hidden");
      box.classList.add("pass");
      box.innerHTML = `<b>Draft saved in this browser session.</b> It is not on chain, and it moves no money.`;
      document.getElementById("draftJson").classList.remove("hidden");
    });
    document.getElementById("copyDraft").addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(draftJson);
        const box = document.getElementById("draftSaved");
        box.classList.remove("hidden");
        box.classList.add("pass");
        box.textContent = "Draft copied to the clipboard.";
      } catch {
        document.getElementById("draftJson").classList.remove("hidden");
        document.getElementById("draftJson").select();
      }
    });
  }
}

// ----- become a juror -----

function renderJuror() {
  const rows = SAMPLE.jurors.map((j) => `<tr>
      <td>${esc(j.label)}</td>
      <td class="mono">${shortHex(j.key)}</td>
      <td>${fmt(j.bond)} &micro;T</td>
    </tr>`).join("");
  page.innerHTML = `
    <section class="section tight">
      <div class="wrap">
        <div class="sec-head">
          <h2>Become a juror.</h2>
          <p class="sec-note">Three bonded strangers rule each dispute. The bond is what makes the ruling worth trusting.</p>
        </div>
        <div class="cols-3">
          <div class="plain-card"><span class="num">BOND</span><h3>${fmt(BOND_FLOOR)} &micro;T floor</h3><p>At least twice the largest juror fee times the panel size. One slot per address.</p></div>
          <div class="plain-card"><span class="num">FEE</span><h3>${fmt(JUROR_FEE)} &micro;T per dispute</h3><p>Paid to each juror who rules, from the disputed principal, jurors first.</p></div>
          <div class="plain-card"><span class="num">PANEL</span><h3>${PANEL_SIZE} jurors per ruling</h3><p>Drawn from a future epoch hash. Seven on appeal, funded by the appellant.</p></div>
        </div>
        <div class="rui-card mt-24">
          <h3>How it works</h3>
          <div class="steps mt-16">
            <div class="step-card"><div class="num">1</div><h3>Register and post bond</h3><p>One slot per address, bond at or above the floor, held in the court vault.</p></div>
            <div class="step-card"><div class="num">2</div><h3>Get drawn</h3><p>Only drawn jurors can vote, and only once per dispute.</p></div>
            <div class="step-card"><div class="num">3</div><h3>Commit, then reveal</h3><p>Commit a hash of your vote and salt, then reveal. A juror who never reveals is replaced, and the replacement draw commits a new future epoch.</p></div>
            <div class="step-card"><div class="num">4</div><h3>Get paid, or get slashed</h3><p>Majority jurors are paid their fee, and slashed bonds top them up. A juror who votes against the majority forfeits bond.</p></div>
            <div class="step-card"><div class="num">5</div><h3>Refuse when you must</h3><p>Refuse to arbitrate is a valid vote. Evidence is public, so read it before you rule.</p></div>
            <div class="step-card"><div class="num">6</div><h3>Stay honest</h3><p>Two of three jurors can collude, and the draw, the bond, commit-reveal, and the appeal are the mitigations, not a guarantee.</p></div>
          </div>
        </div>
        <div class="rui-card mt-24">
          <h3>Demo pool, seeded with known testers and disclosed</h3>
          <p class="sec-note">The demo pool is small and seeded on purpose, and its size is disclosed here rather than hidden. These are sample testers, not real people, and their keys are sample keys.</p>
          <table class="rui-table mt-16"><thead><tr><th>Tester</th><th>Key</th><th>Bond</th></tr></thead><tbody>${rows}</tbody></table>
        </div>
        <div class="form-actions">
          <button class="rui-btn" type="button" id="registerJuror">Register as a juror</button>
        </div>
        <div class="result-box hidden" id="jurorNote"></div>
      </div>
    </section>`;
  window.scrollTo(0, 0);
  document.getElementById("registerJuror").addEventListener("click", () => {
    const box = document.getElementById("jurorNote");
    box.classList.remove("hidden");
    box.innerHTML = `<b>Registration happens on chain.</b> The live path is not wired yet, so this page cannot register you today. The bond floor is ${fmt(BOND_FLOOR)} micro-Tari, one slot per address, and the fee is ${fmt(JUROR_FEE)} micro-Tari per dispute.`;
  });
}

// ----- router -----

function setActiveNav(route) {
  document.querySelectorAll("[data-nav]").forEach((a) => {
    a.classList.toggle("active", a.dataset.nav === route);
  });
}

function route() {
  const hash = location.hash || "#/";
  const parts = hash.replace(/^#\/?/, "").split("/");
  const name = parts[0] || "";
  if (name === "" ) {
    page.innerHTML = HOME_HTML;
    initHome();
    setActiveNav("home");
    return;
  }
  if (name === "app") {
    renderApp();
    setActiveNav("app");
    return;
  }
  if (name === "dispute") {
    renderDispute(Number(parts[1]));
    setActiveNav("disputes");
    return;
  }
  if (name === "open") {
    renderOpen();
    setActiveNav("home");
    return;
  }
  if (name === "juror") {
    renderJuror();
    setActiveNav("home");
    return;
  }
  page.innerHTML = HOME_HTML;
  initHome();
  setActiveNav("home");
}

window.addEventListener("hashchange", route);

document.querySelector(".nav-toggle").addEventListener("click", (event) => {
  const menu = document.getElementById("navMenu");
  const open = menu.classList.toggle("open");
  event.currentTarget.setAttribute("aria-expanded", String(open));
});
document.getElementById("navMenu").addEventListener("click", (event) => {
  if (event.target.closest("a")) {
    document.getElementById("navMenu").classList.remove("open");
    document.querySelector(".nav-toggle").setAttribute("aria-expanded", "false");
  }
});
document.addEventListener("click", (event) => {
  const link = event.target.closest("[data-home-section]");
  if (!link) return;
  event.preventDefault();
  const section = link.dataset.homeSection;
  const go = () => scrollToSection(section);
  if (location.hash === "#/" || location.hash === "") {
    go();
  } else {
    location.hash = "#/";
    setTimeout(go, 50);
  }
  document.getElementById("navMenu").classList.remove("open");
});

initHome();
route();
