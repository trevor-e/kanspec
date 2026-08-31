---
feature: Login (JWT 24h), lockout after 5 failures, GitHub OAuth
code: [src/auth/**, src/middleware/session.ts]   # unquoted globs stay unquoted
---
# auth

## Rules
- [auth.jwt] Login issues a JWT valid 24h in an httpOnly cookie. {p-02cc}
- [auth.lockout] 5 failed logins within 10m locks the account for 15m. {p-7de2}
