# How to add a court to any Ootle escrow

Any Ootle escrow can ship with the Verdict court built in. The escrow keeps
custody of its own vault at all times. The court issues rulings; the escrow's
own code executes them. This guide is the adapter spec: the interface, the
constructor, the gating, and the rules an escrow must follow to adopt it.

## The three-method interface

```rust
freeze_for_dispute(dispute_id)
execute_ruling(dispute_id, payee_amount)
withdraw_remainder()
```

- `freeze_for_dispute(dispute_id)`: called by the arbitrator component when
  a dispute opens. Freezes the disputed vault so no party can move the
  money while the court rules. Freezing blocks every state change on the
  escrow, including a refund that would drain the vault while the court is
  ruling. A second freeze while frozen is refused.
- `execute_ruling(dispute_id, payee_amount)`: called only by the arbitrator
  component when the court settles. Moves the payee's share out of the
  escrow's own vault exactly once, per the ruling, and returns it as a
  bucket. The ruling outcomes map to the payee amount: pay means the payee amount is
  the ruled share, refund means the payee amount is not positive, split means the
  payee amount is the payee's ruled share. The payee amount must never
  exceed the vault balance, and the ruling executes at most once for a
  dispute. See [SETTLEMENT.md](SETTLEMENT.md) for the outcomes and the
  rule that every settlement path conserves microTari exactly.
- `withdraw_remainder()`: called only by the arbitrator component right
  after `execute_ruling`, in the same settlement. Moves whatever is left
  in the vault to the payer and returns it as a bucket, so a split or a
  refund never leaves funds locked in the vault. It refuses to run before
  a ruling has executed, and it runs at most once. When the ruling paid
  full balance, the remainder is empty and it returns an empty bucket.
  The court deposits the payee bucket to the payee account and the
  remainder bucket to the payer account, both derived by the court from
  the payer and payee keys the escrow holds, never from an address a
  caller supplies, so the vault is empty when the settlement finishes.

## Approval deposits to the payee

When the payer approves a milestone or a payment, the escrow deposits
the released slice straight into the payee's account itself. The payer
passes the payee's account address when calling `approve`, and the escrow
does the deposit. The caller never holds the released bucket, so a payer
cannot approve a milestone and then misroute the money to themselves while
the milestone is permanently marked released. Cancel and refund still
return a bucket to the payer, because the payer is the rightful recipient
there.

## Freezing needs funds at stake

`freeze_for_dispute` refuses an escrow whose vault is empty. A dispute
over an empty vault would burn the payee's dispute right and waste a jury
draw with no funds ever at stake, so the freeze requires a positive vault
balance.

All three methods take the dispute id where one applies, and
`execute_ruling` refuses a dispute id that does not match the frozen dispute.

## Constructor

The escrow constructor takes `arbitrator: ComponentAddress`, the address of
the Verdict court component for the deal. The address is stored immutable:
it is set at creation and can never be changed afterwards. A deal names its
court at creation, and no one can swap in a different court later.

The constructor also takes the deal itself: for a milestone escrow, the
milestone amounts and the deadline epoch; for a single-payment escrow, the
payment amount and the deadline epoch. The constructor rejects a deal with
no milestones, a non-positive amount, a milestone total that overflows, a payee
who is the payer, and a deadline that is not after the creation epoch.

## Gating

Three layers, all required. The pattern is the caller-component rule, the
in-method assert, and the ownerless escrow.

1. **Access rule**: the settlement methods (`freeze_for_dispute`,
   `execute_ruling`, `withdraw_remainder`) accept calls only from
   `rule!(caller_component(arbitrator))`. Only calls from the court
   component named at creation are accepted. The rule is readable on chain.
2. **In-method assert**: each method asserts the caller is the arbitrator
   before doing anything, as defense in depth against any access-rule
   subtlety. In `tari_template_lib` 0.41 there is no direct caller-address
   getter, so the assert takes the form of a caller-component badge check
   for the arbitrator.
3. **Ownerless escrow**: the escrow is created with `OwnerRule::None`.
   There is no owner, no admin method, and no upgrade path, so no
   owner-bypass path exists that could rewrite the rules or drain the
   vault. The verified public statement is: the only component whose calls
   the escrow's settlement method accepts is the court.

## Verification constructors (test only, never ship)

The templates also expose `new_owned` and `new_rule_open`. These exist
only so the engine tests can isolate the security layers: the owned twin
proves the owner bypasses the caller rule, and the rule-open twin proves
the in-method assert rejects a stranger even when the access rule would
allow the call. They are test fixtures, not production constructors. A
real escrow holding real funds must always be created with `new`, which
is ownerless, with the caller rule and the in-method assert on. Never
create a live escrow with `new_owned` or `new_rule_open`.

## Deadline and freeze rules

An escrow that adopts the court must define its deadline semantics in
writing, method by method, and follow them:

- Only refund requires the deadline to have passed. Every other party
  method works before the deadline and stops working after it, so the
  deadline is the point where the deal stops moving forward and the
  payer's refund path opens.
- A frozen dispute blocks every state change: funding, delivery, approval,
  cancel, and refund all refuse while frozen.
- A fully released agreement closes. Once every milestone has been
  released, or the single payment has been released, the agreement is
  complete and no further state change is possible. A ruling that has
  executed, a cancel, and a refund are also terminal.
- Cancel and refund handle the empty vault: cancel on an empty vault
  closes the agreement and returns an empty bucket, and refund on an empty
  vault is refused, since there is nothing to return.

## Arithmetic and the settlement path

- Arithmetic is checked everywhere. Totals, funded totals, and released
  totals use checked addition, and every withdrawal is checked against
  the vault balance before it moves.
- `execute_ruling` and `withdraw_remainder` are the settlement path. They
  withdraw from the vault directly and never reuse a payer-only method,
  so no settlement step depends on the transaction signer. A ruling never
  pays out more than the escrow holds, and the two methods together empty
  the vault, so no settlement leaves funds behind.

## The two escrows that ship with Verdict

Two escrows implement this interface, with different deal shapes, to prove
the interface is general and fits any deal, not a one-off:

- `templates/milestone-escrow`: a client funds an agreement with milestone
  amounts, the freelancer marks each milestone delivered, the client
  approves, and approval releases exactly that milestone's slice.
  Unreleased money is refundable after the deadline.
- `templates/payment-escrow`: one amount, one delivery, the payer approves
  to release, the deadline refunds.

Both are ownerless, both take the arbitrator in the constructor, and both
implement the same three methods with the same gating. Any escrow that
implements the three methods and names the court at creation adopts the
court the same way, whatever deal shape it carries.

## Adoption checklist

1. Add `arbitrator: ComponentAddress` to the constructor, store it
   immutable, and reject a deal that names no court.
2. Add `freeze_for_dispute`, `execute_ruling`, and `withdraw_remainder`
   with the signatures above, restricted by the caller-component rule and the
   in-method assert.
3. Create the component with `OwnerRule::None`, with no admin method and
   no upgrade path.
4. Make a frozen dispute block every state change, make the ruling
   execute at most once, and make `withdraw_remainder` run at most once
   and only after a ruling.
5. Define the deadline semantics in writing, method by method, and test
   each one: refund only after the deadline, every other party method
   only before it.
6. Use checked arithmetic for every total, and check every withdrawal
   against the vault balance.
7. Write engine tests for the adapter: the escrow freezes its own vault
   on `freeze_for_dispute`, moves its own vault exactly once on
   `execute_ruling`, routes the remainder to the payer on
   `withdraw_remainder`, rejects a stranger calling any of the three
   methods, rejects a second `execute_ruling` on the same dispute, and
   rejects `withdraw_remainder` before a ruling or a second time.
