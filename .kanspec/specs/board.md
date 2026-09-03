---
feature: The board and review page served by kanspec up, and the terminal and markdown boards
code: [src/board.rs, src/server.rs, src/cmd/board.rs, src/cmd/up.rs, assets/**]
---
# board

## Rules
- [board.view-only] The server is a view and a poller: every POST calls the same `cmd::*` handler the CLI calls, through a request-scoped `Ctx`; there is no second write path. {pre-kanspec}
- [board.same-model] The browser, terminal and markdown boards render one `BoardModel`; badges come from `derive` and are never guessed. {pre-kanspec}
- [board.routes] axum routes use `{id}`, never `:id`, which panics at `Router::route()`. {pre-kanspec}
- [board.static-export] `review --export` writes the review page as one self-contained, script-less HTML file; comments are taken only on the served page or through the CLI. {pre-kanspec}
- [board.review-queue] The Review-queue tab lists proposals in review with their unresolved counts and the tickets in the review column, and approves through the gated verb. {pre-kanspec}
