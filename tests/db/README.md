# Database checks

```sh
npm ci        # in tests/
node migration-state.ts     # CRM tables, extensions and applied migrations
```

It uses `DATABASE_URL` from the environment, or else from `backend/.env`.
