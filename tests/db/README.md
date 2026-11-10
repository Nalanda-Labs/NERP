# Database checks

```sh
npm ci        # in tests/
node yugabyte-features.ts   # does the database support the indexes NERP uses?
node migration-state.ts     # CRM tables, extensions and applied migrations
```

Both use `DATABASE_URL` from the environment, or else from `backend/.env`.
