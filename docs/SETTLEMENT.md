# Settlement

How a Verdict ruling moves money, and the rule every settlement path must
satisfy. All amounts are in micro-Tari. The principal is the escrow vault
balance at the moment the ruling settles.

## The rule: loser pays from principal

All costs of a dispute settle from the disputed principal, never from outside
it. There is no separate fee pot that can run dry, and no outcome where the
loser walks away while someone else funds the court.

The ruling is the majority of the revealed votes, derived by the court from
the commit-reveal record. No caller supplies the amount. `settle` takes only
the dispute id and the two accounts the money goes to.

## Constants

| Name | Value | Meaning |
|---|---|---|
| Juror fee | 5,000 | Paid to each juror in the ruling majority |
| Panel size | 3 | Jurors drawn for a first-round ruling |
| Appeal panel size | 7 | Jurors drawn for an appeal ruling |
| Appeal fee | 35,000 | Posted by the appellant, funds the appeal jurors |
| Slash | 5,000 | Bond forfeited by one offending juror |
| Bond floor | 30,000 | Twice the largest juror fee times the panel size |
| Minimum draw delay | 1 epoch | Every draw commits to a future epoch at least one epoch ahead |
| Reveal window | 2 epochs | Time a drawn juror has to reveal, counted from the draw epoch |
| Draw domain | 0 first round, 1 appeal | Folded into the panel derivation, so the two rounds never draw the same panel from the same hash |

## Draw rules

Every draw commits to a future epoch hash. The delay passed to
`open_dispute`, `replace_jury`, `open_appeal`, and `replace_appeal_jury`
must be at least one epoch, enforced in the template, so a draw can never
use the current epoch hash, which is already known when the dispute
opens.

The eligible pool must already hold a full panel when a dispute opens:
at least three jurors for a first round, at least seven for an appeal.
The check runs at open and again at draw, so the escrow never freezes
into a dispute that cannot draw, and a draw never returns a short panel.

A missed draw window is rescheduled past the current epoch: the new draw
epoch is the current epoch plus one, never the current epoch itself, so a
rescheduled draw still uses a future epoch hash. The reschedule is
recorded on chain.

The panel derivation folds in a draw domain: 0 for the first round, 1
for the appeal. The same hash, pool, dispute id, and round nonce can
never produce the same panel across the two rounds.

A majority is a strict majority of the revealed votes, with at least a
majority of the panel revealed: two of three for a first round, four of
seven for an appeal. A vote to refuse arbitration is valid, but it never
counts toward an outcome, and a refusing juror is neither paid nor slashed.

## The matrix

P is the principal. F is the fees owed from the principal: one juror fee per
juror in the first-round majority, capped at P. The payee amount is what
`execute_ruling` withdraws; the remainder is what `withdraw_remainder`
returns. The court takes F out of the remainder bucket into its fee vault
before paying the payer, so jurors are paid first.

| Outcome | Payee amount | Payer amount | Juror fees | Conservation |
|---|---|---|---|---|
| Pay | P - F | 0 | F | (P - F) + 0 + F = P |
| Refund | 0 | P - F | F | 0 + (P - F) + F = P |
| Split | half of (P - F), rounded up | half of (P - F), rounded down | F | ceil + floor + F = P |

Pay means the payer's side funds the costs: the payee's award is reduced by
the fees. Refund means the principal returns to the payer less the fees.
Split means the post-fee principal is halved, costs pro-rata, with the odd
micro-Tari going to the payee. Every path asserts the conservation equation
in the template: payee plus payer plus fees equals the principal exactly.

Worked example, principal 500,000, two majority jurors, one minority juror:
fees F are 10,000. Pay pays the payee 490,000 and the payer nothing. Refund
pays the payer 490,000 and the payee nothing. Split pays 245,000 each.

Worked example with dust, principal 100,001, two majority jurors: fees F
are 10,000, the post-fee principal is 90,001. Split pays the payee 45,001
and the payer 45,000. The odd micro-Tari goes to the payee.

When the principal is smaller than the fees owed, the fees are capped at
the principal: the payee and the payer both keep nothing, and the majority
jurors split the capped fees by largest remainder. Costs never exceed the
principal.

## Juror fees and slashing

Each juror in the ruling majority is owed one juror fee, plus a share of the
forfeited bonds. The forfeited bonds are split among the majority jurors by
largest remainder: every majority juror gets the floor share, and the first
jurors in panel order get one extra micro-Tari until the remainder runs out.

A juror forfeits one juror fee of bond when they are drawn and do not reveal
a vote, or reveal a vote against the majority. The forfeit is capped by the
bond the juror still holds. A juror who votes to refuse arbitration forfeits
nothing. A juror who fails to reveal in a replaced round is slashed once,
even if the replacement draws them again and they vote with the new
majority.

A juror who is in the majority of both rounds is owed the fee for each
round, and the two claims are added into one. Fees are claimed with
`claim_fee`, once per juror per dispute, into an account the juror names.
A juror who is owed nothing cannot claim.

## Appeal

One appeal per dispute, opened by a party to the dispute, only after the
first round has reached a majority ruling and only before the dispute
settles. The appellant posts exactly the appeal fee, 35,000, in TARI. The
appeal draws seven jurors from the same eligible pool, in a fresh
future-epoch draw with a new commitment and a fresh round nonce.

The appeal ruling settles the dispute. The appeal majority jurors are paid
one juror fee each from the appeal fee, plus a share of the bonds forfeited
by the appeal minority and appeal non-revealers. The first-round majority
jurors are still paid their fee from the principal: the panel whose ruling
settles the dispute is paid, and so is the panel whose ruling was appealed.
The unspent appeal fee returns to the appellant at settle: the appeal fee
less one juror fee per appeal majority juror.

Worked example, principal 500,000, first round rules pay two to one, appeal
rules refund four to three: the payer gets 490,000 of principal back, the
payee gets nothing, the appellant gets 15,000 of appeal fee back, the two
first-round majority jurors are owed 7,500 each, and the four appeal
majority jurors are owed 8,750 each.

An appeal round has the same replacement draw as the first round: one
replacement, committed at the moment the failure is recorded.

## Replacement draw

When a drawn round cannot produce a ruling, because some drawn juror has
not revealed and no outcome holds a majority, the court records the failure
once and commits a new future epoch for a replacement draw. The jurors who
did not reveal are recorded as failed. The panel, the commits, and the
reveals are cleared, the round nonce advances, and the next draw executes
only in the new locked epoch. One replacement per round.

Only a party to the dispute, the payer or the payee, can call a
replacement, and only once the reveal window has passed: the draw epoch
plus two epochs. A replacement can never land while honest jurors are
still within their time to reveal. The deadline is stored on the dispute
at draw time and is readable on chain.

A missed draw window is rescheduled the same way: if the locked epoch
passes without a draw, the commitment moves past the current epoch, and
the reschedule is recorded on chain.

## No-ruling fallback

The bounded exit of the ladder. A dispute closes with no ruling when no
ruling can be reached: either the replacement draw has been used and the new
round still has no majority, or a round is fully revealed with no majority.
The same rule applies to the appeal round.

The close executes a ruling with a payee amount of 0, which unfreezes the escrow, and the whole
principal returns to the payer: the opener's fee is refunded in full, and
no juror fee is taken. Jurors who were drawn and did not reveal are
slashed, and the forfeited bonds go to the jurors who did reveal, split by
largest remainder. When nobody revealed, nobody is slashed: there is no one
to pay, and the court keeps nothing. An unspent appeal fee returns to the
appellant in full. A closed dispute can never settle, and the escrow is
terminal: completed, cancelled, refunded, or ruled with the remainder
withdrawn.

## The ladder, in order

1. First round: draw, commit, reveal, settle.
2. If the round fails, one replacement draw, then settle.
3. If a ruling is reached and a party appeals, appeal draw, commit, reveal,
   settle.
4. If the appeal round fails, one appeal replacement draw, then settle.
5. If no round can reach a majority, close with no ruling.

No step can repeat forever, and no step can leave the escrow frozen. The
court never freezes money indefinitely.

## Invariants

1. The escrow's vault moves only inside the escrow's own code, only on a
   majority or appeal ruling delivered via `execute_ruling`, callable only
   by the arbitrator component.
2. A juror votes at most once per dispute, and only if drawn.
3. Every settlement path conserves micro-Tari exactly; the template never
   pays out more than it holds.
4. A ruling executes at most once.
5. An evidence hash, once posted, cannot be changed.
6. The jury draw is recomputable from published inputs: future epoch hash,
   pool list, dispute id, round nonce, and draw domain.
7. No owner, no admin method, no upgrade, on the court or either escrow.
8. The court can always be exited: ruling, appeal, or bounded no-ruling
   fallback; the court never freezes money indefinitely.
