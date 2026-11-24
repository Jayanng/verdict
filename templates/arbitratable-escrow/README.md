# Reference adapter

This template is the reference implementation of the adapter interface in
[docs/ADAPTER-SPEC.md](../../docs/ADAPTER-SPEC.md). It is based on PieceWork,
a milestone escrow for freelance work on Tari Ootle.

The upstream PieceWork source lives in the fork at
https://github.com/Jayanng/piecework. The adapter work ports that escrow into
this workspace, keeps it ownerless, and adds the two settlement methods
(`freeze_for_dispute` and `execute_ruling`) restricted to the court named at
creation.
