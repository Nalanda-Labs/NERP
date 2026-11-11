// Removes what test runs left in a development database. Test data is recognised by the
// stamp the suites put in names (an 8-character base-36 timestamp, e.g. "Globex mw7h5iyt")
// and in test employees' addresses (name.<stamp>@acme.test). Used by every e2e suite for its
// own stamp at the end of a run, and by cleanup-test-data.ts for everything at once.
//
// It goes through the API (as admin@acme.test, or ADMIN_EMAIL / ADMIN_PASSWORD) wherever the
// app can delete something, so files and links are cleaned up the normal way; notifications
// and the test employees themselves are removed with SQL (the app only deactivates people).
// Never point it at a database with real data in it.
import pg from 'pg';
import { readFileSync } from 'node:fs';

const B = (process.env.NERP_URL ?? 'http://localhost:8080') + '/api';
export const dbUrl = process.env.DATABASE_URL ?? readFileSync(new URL('../../backend/.env', import.meta.url), 'utf8').match(/^DATABASE_URL=(.*)$/m)?.[1];

// A stamp is 8 base-36 characters with a letter, decoding to a time since 2025.
const FROM = Date.UTC(2025, 0, 1), TO = Date.now() + 86_400_000;
const isStamp = (t) => /[a-z]/.test(t) && /\d/.test(t) && (() => { const n = parseInt(t, 36); return n >= FROM && n <= TO; })();
// The stamp may be glued to a word ("Gibbonsmw7h5iyt"), so every 8-character window counts.
const anyStamp = (s) => (s ?? '').toLowerCase().match(/[0-9a-z]{8,}/g)?.some((run) => [...Array(run.length - 7).keys()].some((i) => isStamp(run.slice(i, i + 8)))) ?? false;
const STAMP_SQL = "~ '[0-9a-z]{8}'"; // narrows in SQL; isStamp decides

/**
 * Removes test data: everything carrying `stamp` (one test run's), or every stamp when none
 * is given. With `yes: false` it only reports what it would remove.
 */
export async function cleanup({ yes = true, stamp = null } = {}) {
  const stamped = stamp ? (s) => (s ?? '').toLowerCase().includes(stamp) : anyStamp;
  const isTestUser = (email) => stamp ? email.includes(`.${stamp}@`) : /^[^@]+\.([0-9a-z]{8})@acme\.test$/.test(email) && isStamp(email.split('@')[0].split('.').pop());
  const db = new pg.Client({ connectionString: dbUrl });
  await db.connect();
  let token: any = null;
  async function call(method, path) {
    const res = await fetch(B + path, { method, headers: token ? { authorization: `Bearer ${token}` } : {} });
    return res.status;
  }
  if (yes) {
    const res = await fetch(B + '/auth/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: process.env.ADMIN_EMAIL ?? 'admin@acme.test', password: process.env.ADMIN_PASSWORD ?? 'supersecret1' }),
    });
    token = (await res.json()).access_token;
    if (!token) throw new Error('could not sign in as the admin');
  }
  const rows = async (sql: string, args: unknown[] = []): Promise<any[]> => (await db.query(sql, args)).rows;
  const report: string[] = [];
  async function each(label: string, list: any[], fn: (x: any) => Promise<number | undefined>) {
    report.push(`${String(list.length).padStart(5)}  ${label}`);
    if (!yes) return;
    let failed = 0;
    for (const x of list) {
      const s = await fn(x);
      if (s && s >= 400 && s !== 404) failed++;
    }
    if (failed) report.push(`       ${failed} of them could not be removed`);
  }

  // Test employees.
  const users = (await rows(`SELECT id, email FROM users WHERE email ${STAMP_SQL}`)).filter((u) => isTestUser(u.email));
  const userIds = users.map((u) => u.id);

  // CRM records (also those in the recycle bin), deals first so companies are free last.
  // Records named by tests, and anything a test employee created.
  const items = (await rows(`SELECT i.id, t.code, i.title, i.company_name, i.deleted_at IS NOT NULL AS deleted, i.created_by = ANY($1) AS theirs
                             FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type
                             WHERE i.title ${STAMP_SQL} OR i.company_name ${STAMP_SQL} OR i.created_by = ANY($1)`, [userIds])).filter((i) => i.theirs || stamped(i.title) || stamped(i.company_name));
  const order = (c) => (c === 'contact' ? 1 : c === 'company' ? 2 : 0);
  items.sort((a, b) => order(a.code) - order(b.code));

  // Accounting data of the tests. The app keeps the books for good (corrections are reversing
  // entries), so it is removed here, before the CRM records it refers to (which the app keeps
  // in the recycle bin while the books refer to them).
  {
    const itemIds = items.map((i) => i.id);
    const accts = (await rows(`SELECT id, code, name FROM acc_accounts WHERE code ${STAMP_SQL} OR name ${STAMP_SQL}`)).filter((a) => stamped(a.code) || stamped(a.name)).map((a) => a.id);
    const taxes = (await rows(`SELECT id, name FROM acc_taxes WHERE name ${STAMP_SQL}`)).filter((t) => stamped(t.name)).map((t) => t.id);
    const payments = (await rows(`SELECT id FROM acc_payments WHERE party_id = ANY($1) OR account_id = ANY($2) OR created_by = ANY($3)`, [itemIds, accts, userIds])).map((r) => r.id);
    const bills = (await rows(`SELECT id FROM acc_bills b WHERE party_id = ANY($1) OR created_by = ANY($2) OR EXISTS (SELECT 1 FROM acc_bill_lines l WHERE l.bill_id = b.id AND (l.account_id = ANY($3) OR l.tax_id = ANY($4)))`, [itemIds, userIds, accts, taxes])).map((r) => r.id);
    const bankLines = (await rows(`SELECT id FROM acc_bank_lines WHERE account_id = ANY($1) OR imported_by = ANY($2)`, [accts, userIds])).map((r) => r.id);
    let entries = (await rows(
      `SELECT DISTINCT e.id FROM acc_entries e WHERE e.created_by = ANY($1)
          OR EXISTS (SELECT 1 FROM acc_lines l WHERE l.entry_id = e.id AND (l.party_id = ANY($2) OR l.account_id = ANY($3) OR l.tax_id = ANY($4)))
          OR (e.source = 'invoice' AND e.source_id = ANY($2)) OR (e.source = 'payment' AND e.source_id = ANY($5))
          OR (e.source = 'bill' AND e.source_id = ANY($6)) OR (e.source = 'bank' AND e.source_id = ANY($7))`,
      [userIds, itemIds, accts, taxes, payments, bills, bankLines])).map((r) => r.id);
    // With the entries they reverse and that reverse them.
    for (let grew = true; grew;) {
      const more = (await rows(`SELECT id FROM acc_entries WHERE (reverses = ANY($1) OR reversed_by = ANY($1)) AND NOT id = ANY($1)`, [entries])).map((r) => r.id);
      grew = more.length > 0;
      entries = entries.concat(more);
    }
    const lock = (await rows(`SELECT lock_date FROM acc_settings WHERE id = 1 AND lock_date < DATE '2005-01-01'`)).length;
    report.push(`${String(entries.length).padStart(5)}  accounting entries (with ${bills.length} bills, ${payments.length} payments, ${bankLines.length} statement lines, ${accts.length} accounts, ${taxes.length} taxes)`);
    if (lock) report.push('    1  accounting lock date left by a test');
    if (yes && (entries.length || bills.length || payments.length || bankLines.length || accts.length || taxes.length || lock)) {
      await db.query('BEGIN');
      try {
        await db.query(`UPDATE acc_settings SET
            receivable_account = CASE WHEN receivable_account = ANY($1) THEN NULL ELSE receivable_account END,
            payable_account = CASE WHEN payable_account = ANY($1) THEN NULL ELSE payable_account END,
            sales_account = CASE WHEN sales_account = ANY($1) THEN NULL ELSE sales_account END,
            retained_earnings = CASE WHEN retained_earnings = ANY($1) THEN NULL ELSE retained_earnings END,
            opening_balances = CASE WHEN opening_balances = ANY($1) THEN NULL ELSE opening_balances END,
            fx_account = CASE WHEN fx_account = ANY($1) THEN NULL ELSE fx_account END,
            rounding_account = CASE WHEN rounding_account = ANY($1) THEN NULL ELSE rounding_account END,
            lock_date = CASE WHEN lock_date < DATE '2005-01-01' THEN NULL ELSE lock_date END WHERE id = 1`, [accts]);
        await db.query(`DELETE FROM acc_bank_lines WHERE id = ANY($1) OR line_id IN (SELECT id FROM acc_lines WHERE entry_id = ANY($2))`, [bankLines, entries]);
        await db.query(`DELETE FROM acc_allocations WHERE payment_id = ANY($1) OR doc_id = ANY($2) OR doc_id = ANY($3)`, [payments, itemIds, bills]);
        await db.query(`DELETE FROM acc_payments WHERE id = ANY($1) OR entry_id = ANY($2)`, [payments, entries]);
        await db.query(`DELETE FROM acc_invoices WHERE item_id = ANY($1) OR party_id = ANY($1) OR entry_id = ANY($2)`, [itemIds, entries]);
        await db.query(`DELETE FROM acc_bills WHERE id = ANY($1) OR entry_id = ANY($2)`, [bills, entries]);
        await db.query(`UPDATE acc_entries SET reverses = NULL, reversed_by = NULL WHERE id = ANY($1)`, [entries]);
        await db.query(`DELETE FROM acc_entries WHERE id = ANY($1)`, [entries]);
        await db.query(`DELETE FROM acc_product_accounts WHERE income_account_id = ANY($1)`, [accts]);
        await db.query(`DELETE FROM acc_taxes WHERE id = ANY($1)`, [taxes]);
        await db.query(`UPDATE acc_accounts SET parent_id = NULL WHERE parent_id = ANY($1)`, [accts]);
        await db.query(`DELETE FROM acc_accounts WHERE id = ANY($1)`, [accts]);
        await db.query('COMMIT');
      } catch (e: any) {
        await db.query('ROLLBACK');
        report.push(`       accounting data not removed: ${e.message}`);
      }
    }
  }
  await each('CRM records', items, async (i) => {
    if (!i.deleted) await call('DELETE', `/crm/${i.code}/items/${i.id}`);
    return call('DELETE', `/crm/recycle-bin/${i.id}`);
  });

  // Tasks and projects named by tests or belonging to test employees.
  // Task templates named by tests or made by test employees (removed before tasks, so
  // recurring ones stop making new tasks).
  const templates = (await rows(`SELECT id, title, created_by = ANY($1) AS theirs FROM task_templates`, [userIds])).filter((t) => t.theirs || stamped(t.title));
  await each('task templates', templates, (t) => call('DELETE', `/task-templates/${t.id}`));
  const projects = (await rows(`SELECT id, name, created_by = ANY($1) AS theirs FROM projects`, [userIds])).filter((p) => p.theirs || stamped(p.name));
  const projectIds = projects.map((p) => p.id);
  // A test project's tasks go with it, whatever they are called.
  const tasks = (await rows(`SELECT id, title, created_by = ANY($1) OR responsible_id = ANY($1) OR project_id = ANY($2) OR template_id = ANY($3) AS theirs FROM tasks`, [userIds, projectIds, templates.map((t) => t.id)])).filter((t) => t.theirs || stamped(t.title));
  await each('tasks', tasks, (t) => call('DELETE', `/tasks/${t.id}`));
  await each('projects', projects, (p) => call('DELETE', `/projects/${p.id}`));
  {
    // Booking made by tests: bookings for stamped clients or on stamped resources, then the
    // forms, services and resources themselves.
    const res = (await rows(`SELECT id, name, user_id = ANY($1) AS theirs FROM booking_resources`, [userIds])).filter((r) => r.theirs || stamped(r.name));
    const bks = (await rows(`SELECT b.id, b.client_name AS name, EXISTS (SELECT 1 FROM booking_resources_used u WHERE u.booking_id = b.id AND u.resource_id = ANY($1)) AS theirs FROM bookings b`, [res.map((r) => r.id)])).filter((b) => b.theirs || stamped(b.name));
    const forms = (await rows(`SELECT id, name FROM booking_forms`)).filter((f) => stamped(f.name));
    const svcs = (await rows(`SELECT id, name FROM booking_services`)).filter((x) => stamped(x.name));
    report.push(`${String(bks.length).padStart(5)}  bookings`);
    report.push(`${String(res.length + forms.length + svcs.length).padStart(5)}  booking resources, services and forms`);
    if (yes) {
      if (bks.length) await db.query('DELETE FROM bookings WHERE id = ANY($1)', [bks.map((b) => b.id)]);
      if (forms.length) await db.query('DELETE FROM booking_forms WHERE id = ANY($1)', [forms.map((f) => f.id)]);
      if (svcs.length) await db.query('DELETE FROM booking_services WHERE id = ANY($1)', [svcs.map((x) => x.id)]);
      if (res.length) await db.query('DELETE FROM booking_resources WHERE id = ANY($1)', [res.map((r) => r.id)]);
    }
  }
  {
    // Training courses made by tests (their lessons, tests, attempts and certificates go with them).
    const courses = (await rows(`SELECT id, title FROM training_courses`)).filter((c) => stamped(c.title));
    report.push(`${String(courses.length).padStart(5)}  training courses`);
    if (yes && courses.length) await db.query('DELETE FROM training_courses WHERE id = ANY($1)', [courses.map((c) => c.id)]);
  }
  {
    // Meetings: stamped titles or organised by test employees (their agendas, people and
    // reports go with them), then topics left on no agenda.
    const meetings = (await rows(`SELECT id, title, created_by = ANY($1) AS theirs FROM meetings`, [userIds])).filter((m) => m.theirs || stamped(m.title));
    report.push(`${String(meetings.length).padStart(5)}  meetings`);
    if (yes && meetings.length) {
      await db.query('UPDATE meetings SET parent_id = NULL WHERE parent_id = ANY($1)', [meetings.map((m) => m.id)]);
      await db.query('DELETE FROM meetings WHERE id = ANY($1)', [meetings.map((m) => m.id)]);
      await db.query('DELETE FROM meeting_topics t WHERE NOT EXISTS (SELECT 1 FROM meeting_agenda a WHERE a.topic_id = t.id)');
    }
  }
  {
    // Help desk: tests' tickets (stamped titles or customer addresses, or asked by test
    // employees; their messages go with them), then stamped lists, SLAs and answers.
    const tickets = (await rows(`SELECT id, title, requester_email, requester_id = ANY($1) AS theirs FROM support_tickets`, [userIds]))
      .filter((t) => t.theirs || stamped(t.title) || stamped(t.requester_email));
    report.push(`${String(tickets.length).padStart(5)}  help-desk tickets`);
    if (yes && tickets.length) await db.query('DELETE FROM support_tickets WHERE id = ANY($1)', [tickets.map((t) => t.id)]);
    const entries = (await rows(`SELECT id, name FROM support_dictionary`)).filter((e) => stamped(e.name));
    const slas = (await rows(`SELECT id, name FROM support_slas WHERE NOT is_default`)).filter((s) => stamped(s.name));
    const answers = (await rows(`SELECT id, title FROM support_answers`)).filter((a) => stamped(a.title));
    report.push(`${String(entries.length + slas.length + answers.length).padStart(5)}  help-desk lists, SLAs and answers`);
    if (yes) {
      if (slas.length) await db.query('DELETE FROM support_slas WHERE id = ANY($1)', [slas.map((s) => s.id)]);
      if (entries.length) await db.query('DELETE FROM support_dictionary WHERE id = ANY($1)', [entries.map((e) => e.id)]);
      if (answers.length) await db.query('DELETE FROM support_answers WHERE id = ANY($1)', [answers.map((a) => a.id)]);
    }
  }
  {
    // Deleted tasks wait in the recycle bin: empty it of the tests' ones.
    const binned = (await rows(`SELECT id, title, created_by = ANY($1) OR responsible_id = ANY($1) OR deleted_by = ANY($1) OR project_id = ANY($2) AS theirs FROM task_trash`, [userIds, projectIds])).filter((t) => t.theirs || stamped(t.title));
    report.push(`${String(binned.length).padStart(5)}  tasks in the recycle bin`);
    if (yes && binned.length) await db.query('DELETE FROM task_trash WHERE id = ANY($1)', [binned.map((t) => t.id)]);
  }
  {
    // Task fields made by tests.
    const defs = (await rows(`SELECT id, name FROM task_field_defs`)).filter((d) => stamped(d.name));
    report.push(`${String(defs.length).padStart(5)}  task fields`);
    if (yes && defs.length) await db.query('DELETE FROM task_field_defs WHERE id = ANY($1)', [defs.map((d) => d.id)]);
  }

  // Configuration made by tests.
  const named = async (sql: string) => (await rows(sql)).filter((r) => stamped(r.name));
  await each('pipelines', await named(`SELECT id, name FROM crm_categories WHERE name ${STAMP_SQL}`), (c) => call('DELETE', `/crm/categories/${c.id}`));
  await each('document templates', await named(`SELECT id, name FROM crm_doc_templates WHERE name ${STAMP_SQL}`), (t) => call('DELETE', `/crm/doc-templates/${t.id}`));
  {
    // Stock documents and warehouses made by tests (SQL: posted documents are kept by the app).
    const docs = (await rows(`SELECT id, comment, reference, created_by = ANY($1) AS theirs FROM inv_documents WHERE comment ${STAMP_SQL} OR reference ${STAMP_SQL} OR created_by = ANY($1)`, [userIds])).filter((d) => d.theirs || stamped(d.comment) || stamped(d.reference));
    const whs = (await rows(`SELECT id, name, is_default FROM inv_warehouses WHERE name ${STAMP_SQL}`)).filter((w) => stamped(w.name));
    report.push(`${String(docs.length).padStart(5)}  stock documents`);
    report.push(`${String(whs.length).padStart(5)}  warehouses`);
    if (yes && (docs.length || whs.length)) {
      await db.query('DELETE FROM inv_documents WHERE id = ANY($1)', [docs.map((d) => d.id)]);
      if (whs.some((w) => w.is_default)) {
        await db.query(`UPDATE inv_warehouses SET is_default = FALSE WHERE id = ANY($1)`, [whs.map((w) => w.id)]);
        await db.query(`UPDATE inv_warehouses SET is_default = TRUE WHERE id = (SELECT id FROM inv_warehouses WHERE NOT id = ANY($1) AND is_active ORDER BY created_at LIMIT 1)`, [whs.map((w) => w.id)]);
      }
      await db.query('UPDATE shop_settings SET warehouse_id = NULL WHERE warehouse_id = ANY($1)', [whs.map((w) => w.id)]);
      // Stock the test warehouses held leaves the products' totals.
      const touched = (await db.query('SELECT DISTINCT product_id FROM inv_stock WHERE warehouse_id = ANY($1)', [whs.map((w) => w.id)])).rows.map((r) => r.product_id);
      await db.query('DELETE FROM inv_warehouses WHERE id = ANY($1)', [whs.map((w) => w.id)]);
      await db.query('UPDATE crm_products p SET stock = (SELECT COALESCE(SUM(quantity), 0) FROM inv_stock s WHERE s.product_id = p.id) WHERE id = ANY($1) AND stock IS NOT NULL', [touched]);
    }
  }
  await each('products', await named(`SELECT id, name FROM crm_products WHERE name ${STAMP_SQL}`), (p) => call('DELETE', `/crm/products/${p.id}`));
  for (const [label, table, col] of [['HR document templates', 'hr_doc_templates', 'name'], ['shifts', 'hr_shifts', 'name'], ['saved chat replies', 'chat_canned', 'shortcut'], ['boards', 'boards', 'title'], ['BI dashboards', 'bi_dashboards', 'title'], ['BI datasets', 'bi_datasets', 'name'], ['BI databases', 'bi_sources', 'name'], ['join links', 'join_links', 'note']]) {
    const found = (await rows(`SELECT id, ${col} AS name FROM ${table} WHERE ${col} ${STAMP_SQL}`)).filter((r) => stamped(r.name));
    report.push(`${String(found.length).padStart(5)}  ${label}`);
    if (yes && found.length) await db.query(`DELETE FROM ${table} WHERE id = ANY($1)`, [found.map((f) => f.id)]);
  }
  {
    const evs = (await rows(`SELECT id, title AS name FROM cal_events WHERE title ${STAMP_SQL}`)).filter((r) => stamped(r.name));
    report.push(`${String(evs.length).padStart(5)}  calendar events`);
    if (yes && evs.length) await db.query('DELETE FROM cal_events WHERE id = ANY($1)', [evs.map((e) => e.id)]);
  }
  {
    // Messenger chats named by tests or with a test employee in them (not the general chat
    // or department chats, which everyone is in, unless a test named them).
    const chats = (await rows(`SELECT c.id, c.title, c.audience = 'manual' AND EXISTS (SELECT 1 FROM im_members m WHERE m.chat_id = c.id AND m.user_id = ANY($1)) AS theirs FROM im_chats c`, [userIds])).filter((c) => c.theirs || stamped(c.title));
    report.push(`${String(chats.length).padStart(5)}  messenger chats`);
    if (yes && chats.length) await db.query('DELETE FROM im_chats WHERE id = ANY($1)', [chats.map((c) => c.id)]);
  }
  {
    const posts = (await rows(`SELECT id, body, author_id = ANY($1) AS theirs FROM feed_posts WHERE body ${STAMP_SQL} OR author_id = ANY($1)`, [userIds])).filter((p) => p.theirs || stamped(p.body));
    report.push(`${String(posts.length).padStart(5)}  news feed posts`);
    if (yes && posts.length) await db.query('DELETE FROM feed_posts WHERE id = ANY($1)', [posts.map((p) => p.id)]);
  }
  {
    const spaces = (await rows(`SELECT id, name FROM kb_spaces WHERE name ${STAMP_SQL}`)).filter((r) => stamped(r.name));
    report.push(`${String(spaces.length).padStart(5)}  knowledge base spaces`);
    if (yes && spaces.length) await db.query('DELETE FROM kb_spaces WHERE id = ANY($1)', [spaces.map((r) => r.id)]);
  }
  {
    const hol = (await rows(`SELECT day::text AS id, name FROM hr_holidays WHERE name ${STAMP_SQL}`)).filter((r) => stamped(r.name));
    report.push(`${String(hol.length).padStart(5)}  holidays`);
    if (yes && hol.length) await db.query('DELETE FROM hr_holidays WHERE day::text = ANY($1)', [hol.map((h) => h.id)]);
  }
  {
    const nodes = (await rows(`SELECT id, name, created_by = ANY($1) OR owner_id = ANY($1) AS theirs FROM drive_nodes WHERE parent_id IS NULL`, [userIds])).filter((n) => n.theirs || stamped(n.name));
    report.push(`${String(nodes.length).padStart(5)}  drive items`);
    if (yes && nodes.length) {
      const files = (await db.query(`WITH RECURSIVE down AS (SELECT id FROM drive_nodes WHERE id = ANY($1) UNION ALL SELECT d.id FROM drive_nodes d JOIN down ON d.parent_id = down.id)
                                     SELECT file_id FROM drive_versions WHERE node_id IN (SELECT id FROM down) AND file_id IS NOT NULL`, [nodes.map((n) => n.id)])).rows.map((r) => r.file_id);
      await db.query('DELETE FROM drive_nodes WHERE id = ANY($1)', [nodes.map((n) => n.id)]);
      for (const f of files) await call('DELETE', `/files/${f}`);
    }
  }
  await each('lists', await named(`SELECT id, name FROM lists WHERE name ${STAMP_SQL}`), (x) => call('DELETE', `/lists/${x.id}`));
  await each('websites', await named(`SELECT id, name FROM site_sites WHERE name ${STAMP_SQL}`), (x) => call('DELETE', `/sites/${x.id}`));
  await each('chat buttons', await named(`SELECT id, name FROM chat_channels WHERE name ${STAMP_SQL}`), (c) => call('DELETE', `/chat/channels/${c.id}`));
  await each('product properties', await named(`SELECT id, name FROM crm_product_props WHERE name ${STAMP_SQL}`), (x) => call('DELETE', `/crm/product-props/${x.id}`));
  await each('price types', await named(`SELECT id, name FROM crm_price_types WHERE name ${STAMP_SQL}`), (x) => call('DELETE', `/crm/price-types/${x.id}`));
  await each('shop categories', await named(`SELECT id, name FROM shop_categories WHERE name ${STAMP_SQL}`), (c) => call('DELETE', `/shop/categories/${c.id}`));
  await each('files', await named(`SELECT id, original_name AS name FROM files WHERE original_name ${STAMP_SQL}`), (f) => call('DELETE', `/files/${f.id}`));
  await each('web forms', await named(`SELECT id, name FROM crm_webforms WHERE name ${STAMP_SQL}`), (f) => call('DELETE', `/crm/webforms/${f.id}`));
  await each('custom fields', await named(`SELECT f.id, f.label AS name, t.code FROM crm_field_defs f JOIN crm_entity_types t ON t.id = f.entity_type WHERE f.label ${STAMP_SQL}`), (f) => call('DELETE', `/crm/${f.code}/fields/${f.id}`));
  await each('smart processes', (await rows(`SELECT code, name_plural AS name FROM crm_entity_types WHERE NOT is_system AND kind IS NULL`)).filter((t) => stamped(t.name)), (t) => call('DELETE', `/crm/types/${t.code}`));
  await each('email templates', await named(`SELECT id, name FROM crm_email_templates WHERE name ${STAMP_SQL}`), (t) => call('DELETE', `/crm/email-templates/${t.id}`));
  // Sub-departments first: a department with sub-departments can't be deleted.
  await each('departments', await named(`WITH RECURSIVE depth AS (SELECT id, 0 AS n FROM departments WHERE parent_id IS NULL
                                           UNION ALL SELECT d.id, depth.n + 1 FROM departments d JOIN depth ON d.parent_id = depth.id)
                                         SELECT d.id, d.name FROM departments d JOIN depth USING (id) WHERE d.name ${STAMP_SQL} ORDER BY depth.n DESC`), (d) => call('DELETE', `/departments/${d.id}`));
  await each('campaigns', await named(`SELECT id, name FROM mkt_campaigns WHERE name ${STAMP_SQL}`), (c) => call('DELETE', `/marketing/campaigns/${c.id}`));
  await each('segments', await named(`SELECT id, name FROM mkt_segments WHERE name ${STAMP_SQL}`), (g) => call('DELETE', `/marketing/segments/${g.id}`));
  {
    const out = (await rows(`SELECT email AS name FROM mkt_optouts WHERE email ${STAMP_SQL}`)).filter((r) => stamped(r.name));
    report.push(`${String(out.length).padStart(5)}  unsubscribed test addresses`);
    if (yes && out.length) await db.query('DELETE FROM mkt_optouts WHERE email = ANY($1)', [out.map((o) => o.name)]);
  }
  await each('webhooks', await named(`SELECT id, name FROM webhook_subscriptions WHERE name ${STAMP_SQL}`), (w) => call('DELETE', `/webhooks/${w.id}`));
  {
    const keys = await named(`SELECT id, name FROM api_keys WHERE name ${STAMP_SQL}`);
    report.push(`${String(keys.length).padStart(5)}  API keys`);
    if (yes && keys.length) await db.query('DELETE FROM api_keys WHERE id = ANY($1)', [keys.map((k) => k.id)]);
  }
  await each('roles', await named(`SELECT id, name FROM roles WHERE name ${STAMP_SQL} AND NOT is_system`), (r) => call('DELETE', `/roles/${r.id}`));

  // Notifications about test data, then the test employees.
  const notes = (await rows(`SELECT id, title, user_id = ANY($1) AS theirs FROM notifications`, [userIds])).filter((n) => n.theirs || stamped(n.title));
  report.push(`${String(notes.length).padStart(5)}  notifications`);
  if (yes) await db.query('DELETE FROM notifications WHERE id = ANY($1)', [notes.map((n) => n.id)]);
  report.push(`${String(users.length).padStart(5)}  test employees`);
  if (yes && users.length) {
    // Background jobs (mail sync, robots) may still be writing for these people for a
    // moment, so a failed attempt is retried.
    let error: any = null;
    for (let attempt = 0; attempt < 4; attempt++) {
      try {
        await db.query('BEGIN');
        // Activities they were responsible for on records that stay go to the admin.
        await db.query(`UPDATE crm_activities SET responsible_id = (SELECT id FROM users WHERE email = $2) WHERE responsible_id = ANY($1)`, [userIds, process.env.ADMIN_EMAIL ?? 'admin@acme.test']);
        await db.query('DELETE FROM tasks WHERE created_by = ANY($1) OR responsible_id = ANY($1)', [userIds]);
        await db.query('DELETE FROM calls WHERE started_by = ANY($1) OR id IN (SELECT call_id FROM call_invites WHERE user_id = ANY($1))', [userIds]);
        await db.query('UPDATE shop_settings SET handlers = ARRAY(SELECT h FROM unnest(handlers) h WHERE NOT h = ANY($1))', [userIds]);
        await db.query('DELETE FROM users WHERE id = ANY($1)', [userIds]);
        await db.query('COMMIT');
        error = null;
        break;
      } catch (e: any) {
        await db.query('ROLLBACK');
        error = e;
        await new Promise((r) => setTimeout(r, 2000));
      }
    }
    if (error) report.push(`       employees not removed: ${error.message}`);
  }

  // Directory and sign-on settings an interrupted directory suite left pointing at its
  // throwaway servers (OpenLDAP on :3893, the mock provider on :9477/:9478).
  const directory = await rows(`SELECT 1 FROM directory_config WHERE ldap ->> 'url' LIKE '%:3893' OR sso ->> 'issuer' ~ 'localhost:947[0-9]$'`);
  if (directory.length) {
    report.push('    1  directory / sign-on test settings');
    if (yes) await db.query(`UPDATE directory_config SET ldap = '{}', ldap_secret = NULL, ldap_synced_at = NULL, ldap_sync_result = NULL, sso = '{}', sso_secret = NULL`);
  }


    await db.end();
    return report;
  }
