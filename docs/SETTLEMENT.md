# Settlement

How a Verdict ruling moves money, and the rule every settlement path must
satisfy.

## The rule: loser pays from principal

All costs of a dispute settle from the disputed principal, never from outside
it. There is no separate fee pot that can run dry, and no outcome where the
loser walks away while someone else funds the court.

## The three outcomes

| Outcome | What happens to the principal | Who funds the costs |
|---|---|---|
| Pay | The milestone amount goes to the payee. | The payer's side funds the costs, taken from the principal. |
| Refund | The milestone amount returns to the payer. | The opener's posted fee stays in the pot and funds the costs. |
| Split | The principal is divided between payer and payee as ruled. | Costs are taken pro-rata from each side's share. |

## Ordering and caps

1. Jurors are paid first. A ruling never pays the parties before the jurors
   who ruled on it.
2. Costs are capped at the principal. Settlement never pays out more than the
   escrow holds.
3. Slashed juror bonds top up the majority jurors. A juror who fails to
   reveal, or votes against the majority after committing, forfeits bond to
   the jurors who carried the ruling.

## Master invariant

**Every settlement path conserves microTari exactly.** For each of the three
outcomes, the amounts paid to the parties plus the amounts paid to jurors
plus any retained fees equal the principal that entered settlement, to the
microTari. Dust is resolved with largest-remainder rounding, and the
conservation check is asserted in the template on every path.

Every cell of the outcome matrix above is covered by a unit test that asserts
conservation.
