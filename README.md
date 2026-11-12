# NERP

[![Build](https://github.com/Nalanda-Labs/NERP/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/Nalanda-Labs/NERP/actions/workflows/build.yml)
[![Tests](https://github.com/Nalanda-Labs/NERP/actions/workflows/tests.yml/badge.svg?branch=main)](https://github.com/Nalanda-Labs/NERP/actions/workflows/tests.yml)

A company management platform (CRM, tasks and projects, documents, email), written from scratch.

| Part     | Stack                                                                         |
|----------|-------------------------------------------------------------------------------|
| backend  | Rust 2024, Axum 0.8, SQLx 0.9, PostgreSQL 18, Redis/Valkey                    |
| frontend | SvelteKit 3 (SPA, static adapter), Svelte 5 runes, TypeScript, Tailwind CSS 4 |

**Guides:** [SETUP.md](SETUP.md) (development and production setup) ·
[BACKUP.md](BACKUP.md) (backups and restoring) · [SECURITY.md](SECURITY.md) (security review,
penetration tests, reporting vulnerabilities)

## Modules

**Core** (done)
- First-run setup: creates the company, its root department and the first administrator
- Sign-in with email and password: a 15-minute JWT access token held in memory, and a 30-day refresh
  token in an httpOnly cookie, stored hashed in Redis and rotated on every use
- Sign-out, password change (signs out other devices), admin password reset
- Brute-force protection: 10 failed sign-ins per email locks it for 15 minutes (Redis)
- Employee directory: search and filters, profiles, avatars, deactivation/reactivation
- Company structure: department tree, department heads, members
- Roles and permissions: a permission catalog defined in code, built-in Administrator/Employee roles,
  custom roles. Each user's permissions are cached in Redis and invalidated whenever they change.
- Files: streamed uploads to local disk, SHA-256 checksums, private or public files
- Company settings
- Audit log of security-relevant actions

The safeguards always keep at least one active administrator, and nobody can deactivate themselves.

**Platform services** (done)
- **Background jobs:** a queue in PostgreSQL, worked on by every instance with `WORKER_ENABLED=true`.
  Failed jobs retry with backoff (up to 5 attempts). If a worker dies, its jobs return to the queue.
  Recurring maintenance (cleaning up old notifications and jobs) runs once per interval across all
  instances, coordinated by a Redis lock. Admins can watch the queue under *System*.
- **Email:** sent over SMTP (none, STARTTLS or TLS) through the job queue, as HTML plus plain text.
  Templates live in `backend/src/mail.rs`.
- **Real-time:** a WebSocket at `/api/realtime/ws`. Events go through Redis pub/sub, so a user
  connected to any instance receives them. Browsers connect with a single-use 30-second ticket.
- **Notifications:** stored in the database and pushed live, with a bell and unread badge, read
  state synced across tabs, and a notifications page. Modules raise them with `notify::notify()`.

**Accounts** (done)
- **Invitations:** an admin enters name and email, and the person sets their own password from the
  emailed link (valid 7 days, can be resent).
- **Forgot password:** a single-use emailed link valid for 1 hour. The response is the same whether
  or not the email exists, requests are rate-limited, and using the link signs the user out everywhere.
- **Sessions:** each signed-in device is a named session showing browser, IP and last activity.
  You can sign out one device or all others, and admins can sign a user out everywhere. A
  revoked device loses access immediately: its access token is rejected and its open tabs are
  told to sign out.
- **Two-factor authentication:** authenticator-app codes (RFC 6238 TOTP), set up with a QR code,
  each code accepted only once, plus 10 single-use recovery codes. Admins can turn it off for a
  user who lost their phone.
- **Required two-step sign-in** (Settings → Security → "Require two-step sign-in for
  everyone"): after their password, LDAP password or company sign-on, everyone must enter a
  code. Anyone who hasn't set it up is stopped at their next sign-in until they enter their
  mobile number (saved on their profile), add the QR code to an authenticator app on that phone,
  and type a code; they then get their recovery codes and are signed in. While it is required,
  nobody can turn their own off. Codes come from the app; nothing is sent by SMS.

**Tasks & Projects** (done)
- **Projects:** private or open to the whole company, with owners, members and viewers. Members
  can join open projects themselves. Projects can be archived, and a project always keeps at
  least one owner.
- **Tasks:** creator, responsible person, assistants and watchers; deadline, priority, estimate,
  subtasks, checklist, file attachments, and consecutive human-friendly numbers (#123).
- **Workflow:** To do → In progress → Done (or Deferred). With *review when done*, finished work
  goes to *Awaiting review* until the creator accepts it or sends it back.
- **Who can do what:**
  - See a task: participants, project members (or everyone, if the project is open), the
    responsible person's department head, and `tasks.manage_all`.
  - Edit or delete: the creator and project owners.
  - Change status, tick the checklist and log time: the responsible person and assistants.
- **Comments with @mentions:** someone mentioned who can't see the task becomes a watcher.
- **Time tracking:** a start/stop timer (one running per person, shown in the header) plus manual
  entries. Finishing a task stops its timers.
- **Views:** list (my tasks, created by me, assisting, watching; filters for open, done or
  overdue; search including `#number`), a Kanban board with drag and drop, and a dashboard widget.
  Changes appear live for everyone who can see them.
- **Notifications** for assignment, being added as an assistant or watcher, comments, mentions,
  review requests, acceptance or return, and deadline changes.
- **Reminders:** a background job notifies people when a deadline is within 24 hours and when a
  task becomes overdue, once each.
- **History:** every change is recorded on the task's History tab.
- **Board columns per project:** owners add, rename, recolour, reorder and delete columns. A
  column can be linked to a status: dropping a card there applies it (a review task dropped on
  Done goes to Review), and a status change elsewhere moves the card to the matching column.
- **Gantt chart and dependencies:** tasks have an optional start date. "Depends on" links are
  finish-to-start, and loops are rejected. Drag a bar to move a task, or drag its right edge to
  change the deadline. Moving a task later pushes the tasks that wait for it later too, through
  the whole chain (only tasks you're allowed to edit), and their owners are notified. The chart
  draws a red dashed arrow where a dependency is violated.
- **Templates and recurring tasks:** save any task as a template, or build one from scratch, and
  create tasks from it with one click. A template can repeat daily, weekly on chosen days, or
  monthly on a given day, at a local time in an IANA time zone (correct across daylight-saving
  changes), with an optional end date or number of repetitions. A background job
  (`tasks.recurring`, every minute) creates each task as the template owner, with the deadline
  measured from the scheduled time.
- **New permissions:** `projects.create` (given to everyone by default) and `tasks.manage_all`.
- **More views:** "My team" (the tasks of everyone in the departments you head, and theirs
  below), "New comments" (tasks with comments by others since you last opened them; the list
  shows "N new"), "Overdue", sorting by deadline, recent activity, newest or priority, a
  **calendar** of tasks by deadline, and **My planner**: your open tasks in columns of your own
  ("Do now", "Next", "Later" to start with) that change nothing for anyone else.
- **Checklist points for people:** a point can be given to someone (they join the task as an
  assistant if needed, and are told).
- **Results and ratings:** any comment can be marked as the task's **result** (shown at the
  top); with "Needs a result" the task can't be finished without one. Whoever set a finished
  task rates it 👍 or 👎, and the responsible person is told.
- **Skip weekends and holidays:** with this option, deadlines and start dates that fall on a
  non-working day (the HR work week and company holidays) move to the next working day.
- **Custom fields:** "Fields" (with "manage all tasks") adds fields on every task; a project's
  owners add fields for its tasks in the project's settings. Text, number, date, list,
  yes/no, employee or link; optionally required (checked in the task form).
- **Recycle bin** ("Bin"): a deleted task waits 30 days with its checklist, comments, files,
  history, time and links, and comes back as it was (its subtasks become its subtasks
  again); its creator, whoever deleted it and "manage all tasks" see it there.
- **Board automation** (⚡ Automation on a project's board, for its owners): *rules* run when
  a task enters a column — tell someone, comment, make someone responsible, add an assistant
  or watcher, set a deadline, the priority or the status, make a subtask (texts may use
  {task}, {number}, {column}, {responsible}); *triggers* move a task to a column when someone
  comments, the deadline passes, the checklist is all ticked or work starts. The editor
  shows what the automation recently did.
- **Templates** can hold subtasks (made under the task each time) and be shared with everyone
  (others use them; only the owner changes them).
- **Copying a project** (Settings → "Copy as a new project"): columns, task fields,
  automation, optionally members, and its tasks as new ones (with checklists and subtasks,
  dates moved with the new start date).
- **Workload and efficiency** (Tasks → Reports): per person over a period — finished, late,
  open, overdue, estimates left, time logged, average time to finish, ratings, and
  efficiency (finished on time out of finished plus overdue). Your own; your team's if you
  head a department; everyone's with "manage all tasks".
- **Flows** (Tasks → Flows, or a group's board): a queue of similar tasks for a team. Anyone
  sends a flow a task; it goes to the next team member in turn, waits for the flow's
  administrator to hand it out, or waits for someone in the team to take it. The flow shows
  what is waiting, at work and done, the average wait before work starts, and its efficiency
  (tasks done within the planned time).

**CRM, phase 1: sales core** (done)
- **One generic record engine** for leads, deals, contacts and companies, so future record types
  (quotes, invoices, smart processes) plug into the same storage, API and UI.
- **Pipelines and stages:** several deal pipelines, each stage open, successful or failed;
  admins add, rename, recolour, reorder and delete stages (records move to a stage of the same
  kind). Lead statuses work the same way.
- **Lead conversion** into a contact, company and/or deal, carrying over names, contact details,
  address, amount and products. Every record links back to its source.
- **Custom fields:** text, long text, number, money, date, date and time, yes/no, list, employee,
  link and address; required, multiple-value and show-in-list options. They can be filtered on.
  A field can be limited to chosen people and departments (with their sub-departments): everyone
  else gets neither the field nor its values — not on the record, list, board, exports,
  documents or e-mail templates, not in the edit history, and not as a filter — and can't write
  it (a required field they can't see isn't asked of them). CRM admins see every field.
- **Contact details:** any number of phones, emails, websites and messengers, each with a type.
- **Every country:** ISO 3166 countries and ISO 4217 currencies. Names, symbols and number formats
  come from the browser's localisation data. Structured addresses.
- **Currencies:** enable any currency, set exchange rates, switch the base currency (rates are
  rescaled so values are kept). Amounts are stored as exact decimals and also converted to the
  base currency for filters and board totals.
- **Products:** a catalog with SKU, unit, price and currency, tax rate (inclusive or exclusive),
  plus product rows on deals and leads with percent or amount discounts and exact tax maths. The
  deal amount follows the products. A product's cost price (or, without one, the average
  purchase price of its posted receipts) is copied onto the rows when they're saved, so later
  changes don't rewrite past deals; a row's cost can also be typed in.
- **Views:** a Kanban board per pipeline (per-stage counts and totals, drag and drop, "show more")
  and a list with column-aware rows, paging, multi-select and bulk assign, change stage or delete.
- **Filters:** search by name, phone, email or #number, mine, state, activity (overdue, today,
  planned, none), responsible, source, dates, amount range and custom fields. Filters can be saved.
- **Timeline** on every record: created, stage changes, assignment, conversion, merge, restore,
  products and activities, plus comments that can be pinned and can carry files (up to 20 per
  comment; whoever can see the record can download them, and deleting the comment removes them).
- **Activities:** to-dos, calls (incoming or outgoing; logged as done if no time is set), meetings
  and emails, with deadlines, reminders (a background job) and a "My activities" page. Sidebar
  counters show deals and leads that need attention: overdue, due today, or nothing planned.
- **Access rights** per role × record type × action (view, add, edit, delete, export, import) with
  scope none, own, department, department and sub-departments, or all. `crm.admin` grants full
  access and the settings; `crm.products` manages the catalog.
- **Data tools:**
  - duplicate warnings while typing (phone, email or name), and merging records
  - **Duplicates** (`/crm/{type}/duplicates`): a search over the whole type for groups of
    records sharing a phone, an email or a name, among the records you may read. Merge chosen
    groups at once (each into its oldest record, or the one you pick), or mark a group "not
    duplicates" so it stays hidden until another record joins it
  - CSV import as a background job, with an auto-guessed column mapping, duplicate handling
    (skip, update or create), progress and an error report. Contacts exported from Gmail,
    Outlook or Yahoo Mail are recognised and mapped as they are (UTF-8, UTF-16 or
    Windows-1252; country names and unknown birthday formats are left out instead of failing
    the row), and vCard files (2.1, 3.0, 4.0) import contacts with their company, phones,
    emails, messengers, address, birthday and notes
  - **Move from another CRM** (CRM → Move from another CRM, CRM administrators): upload the
    export of HubSpot, Salesforce (its data export ZIP), Pipedrive or Zoho CRM, or any CSV
    files. Each file's kind (companies, contacts, leads, deals, activities, notes, products,
    products on deals, attachments, users) and
    each column are recognised from the system's own column names; the page shows what it
    found to check and change: the column mapping, deal pipelines (an existing one, or a new
    one made with the stages the export uses), stages and lead statuses, and whose records
    are whose (matched to employees by email or name, or through Salesforce's users file).
    It runs in the background — companies, contacts, leads, deals, then activities and
    notes — keeping the links between them (contacts to companies, deals to their company
    and contacts, notes and tasks to their record), original creation dates, amounts,
    close dates and probabilities, and notes' HTML as text.
    **Custom fields:** a column named like one of the record type's custom fields goes to it;
    any other column can go to an existing custom field or to a new one, named after the
    column and typed by its values (number, date, date and time, yes/no, link, text); values
    are converted to the field's type (list fields by their option names, people fields by
    the owner matching) and a value that doesn't fit leaves the field empty rather than
    failing the record. **Products:** the export's catalogue comes into the product catalogue
    (matched to existing products by code, else name), and products on deals (Salesforce's
    OpportunityLineItem, HubSpot's line items, …) become the deals' product rows with
    quantity, price, discount and tax; a deal whose export gave no amount takes its rows'
    total. **Attachments:** files inside the uploaded ZIP that the export lists (Salesforce's
    Attachment and ContentVersion, or any list with a parent id and a file path or id) go on
    their record's timeline, kept by whoever owned them; an attachment of a note or task goes
    to that note's or task's record. Nothing is sent and no robots,
    business processes, webhooks or notifications run. Every record's id in the old system is
    remembered, so a second run of the same (or a newer) export updates what came over before
    instead of duplicating it; people and companies already in NERP are matched by email,
    phone or name. A migration can be undone: what it made goes to the recycle bin (products,
    product rows, notes, activities and attachments with their files are removed)
  - a **Booking link** on each record: one of your calendar slot links made out for the
    record's client. What they book is a meeting on the record (and on your calendar, once);
    if they cancel, the meeting is closed as cancelled
  - streamed CSV export, with protection against spreadsheet formula injection; also Excel
    (.xlsx) and, for contacts, leads and companies, vCards (Outlook, Gmail, phones)
  - a chat in each record (its responsible person, creator and observers; opening it as
    someone else who may read the record makes you an observer)
  - the company's own traffic sources (CRM settings → Record types) beside the built-in
    ones: a record arriving with one of a source's utm_source values gets that source
  - document templates limited to chosen people and departments
  - a recycle bin with restore, purged automatically after 30 days (emptying it also removes
    the records' generated documents, and emails no other record shows, with their files)
- **Built for millions of records:** indexes in the order lists are read, a trigram index for
  search, counts capped at 10,000, keyset-paged exports, and rate recalculation in
  batches.

### Documents (CRM phase 2)

Quotes, invoices and other PDFs are generated from templates. Authors pick one of three engines:

| Engine | Template | How it runs |
| --- | --- | --- |
| **Typst** (default) | Typst source, edited in the browser | built into the backend, in-process, about 50 ms per document |
| **LuaLaTeX** | LaTeX source using the `nerp` package | `lualatex` as a separate process, as a background job |
| **LibreOffice** | a Word `.docx` (or Writer `.odt`, converted on upload) with `{{placeholders}}` | NERP fills the DOCX; headless `soffice` converts it to PDF in a background job |

- **The same data for every engine:** seller (Settings → Company details on documents), client,
  record, contacts, product rows, totals and a per-rate tax breakdown, and custom fields. Money,
  numbers and dates are formatted on the server for the template's locale (e.g. `1.234,50 €`,
  `₹1,23,456.50`, `¥1,235`), so all engines print identical figures.
- **Every script:** the built-in templates use Noto font fallback chains (Latin, Cyrillic, Greek,
  Devanagari, Bengali, Arabic, Hebrew, CJK, Thai and more). LaTeX lays out Arabic and Hebrew
  right to left.
- **Safe with any CRM data:** Typst sees only `data.json` and the logo. LaTeX values are printed
  with catcode −2, so `\input`, `%` or `$` in a company name print literally. LaTeX also runs with
  shell escape off and file access limited to its job folder. DOCX values are XML-escaped.
  External processes have a timeout, kill their whole process group, and are capped at
  `DOC_RENDER_CONCURRENCY` at a time.
- **Numbering:** gap-free per prefix (`INV-{YYYY}-0001`; `{YYYY}`, `{YY}` and `{MM}` are
  replaced). Templates that share a prefix share the sequence.
- **CRM → Documents** (`/crm/documents`): every document across records, newest first.
  - **Filters:** search (number, record or company), kind (Invoice, Quote …), record type,
    status and a date range. The filters are kept in the address, so `/crm/documents?kind=Invoice`
    can be bookmarked.
  - **Each row** shows the record, client and record amount, and buttons to open the PDF,
    download it, or download the filled DOCX.
  - **Several PDFs at once:** tick documents (or all on the page) and download them as one ZIP,
    up to 200 at a time, each file named by its document number.
  - **Visibility:** only documents on records the user may view appear in the list or the ZIP.
- **Templates screen** (CRM settings → Documents, `/crm/settings?tab=documents`; every settings
  tab has its own address): a source editor with a clickable field
  reference, a live PDF preview with sample data, and error messages with line numbers (Typst)
  or the TeX log (LaTeX).
  - Any template can be downloaded as a file to edit elsewhere: `.typ`, `.tex` or `.docx`.
  - **Open file** loads a `.typ` or `.tex` file back into the editor. Word templates are
    uploaded as `.docx` or `.odt`. Nothing is stored until you save.
- **On records:** a Documents card to create documents (offered per record type), open or
  download the PDF (and the filled DOCX), regenerate with the current data while keeping the
  number, and delete. Background documents update live. Creating a document adds a timeline
  entry. A PDF is readable by anyone who can view its record.
- Built-ins: Invoice and Quote (Typst), Invoice (LaTeX) and Invoice (Word), plus three example
  invoice designs for each engine, so there are nine in all:
  - **Classic:** a serif letterhead, a ruled table and a signature line.
  - **Modern:** a colour band header, summary cards for date, reference and amount due, and a
    coloured total.
  - **Minimal:** monochrome with hairlines, and the amount due up front.

  The sources are in `backend/assets/doc-templates/` (`invoice-<style>.typ`, `.tex` and
  `.docx`). The Word files are generated by `gen_invoice_docx.py` and `gen_example_docx.py`.
  Engines missing on a server are detected at start-up, hidden in the UI, and shown on the
  System page.
- **When a record has no products,** the invoices print one line with the record title and
  total. Templates do this with `\crmifnot{has_products}{…}` in LaTeX and with
  `{{^products}}…{{/products}}` in Word. `{{^x}}` is the opposite of `{{#x}}`: its content is
  kept only when `x` is empty or false.

### Requisites (legal details)

- **Per country:** companies and contacts can hold one or more sets of legal details: the
  registered name, the country's identifiers, a legal address and bank accounts.
  - 50+ countries have their own field sets, e.g. GSTIN/PAN/CIN for India, USt-IdNr./
    Handelsregister for Germany, SIREN/SIRET/TVA for France, EIN for the US, ABN/ACN for
    Australia, CNPJ/CPF for Brazil, RFC for Mexico, UEN for Singapore, TRN for the UAE, and VAT
    for every EU member.
  - Any other country gets a tax ID and a registration number.
  - Bank fields follow local practice: IBAN/BIC, ABA routing, sort code, IFSC, BSB, CLABE, CBU
    and so on.
- **Checked as you type:** format checks for every structured identifier, plus real checksums
  for IBAN, ABA routing, GSTIN, ABN, CNPJ, INN, NIP and SIREN/SIRET (Luhn).
- **Duplicate warning:** while typing, NERP shows other records with the same number. Only
  records you can see are listed, and only under the same country's scheme.
- **Searchable:** you can find a record by its tax number or legal name. Merging records keeps
  their requisites.
- **Bill to:** deals and leads pick which requisites (of their company or contacts) documents
  are addressed to. The default is the company's default requisite.
  - Documents get `client.legal_name`, `client.tax_line` ("GSTIN … · PAN …"), `client.tax_ids`,
    `client.ids.<code>`, `client.bank_line` and the legal address.
  - The built-in templates print the tax line. Built-in templates no admin has edited are
    upgraded automatically on start.
- **Word templates:** a paragraph wrapped in `{{#x}}…{{/x}}` is left out when `x` is empty.

### Sales analytics

CRM → Analytics (`/crm/analytics`) reports on deals, or on any smart process with stages and
amounts.

- **Filters:** period (this month, last month, last 30 days, this quarter, this year, last
  year, or a custom range), pipeline and managers. The filters are kept in the address, so a
  view can be bookmarked.
- **Headline figures:** amount won, win rate, average won deal, time to win, new deals and the
  open pipeline. Each is compared with the previous period of the same length.
- **Sales over time:** won amount, won count or new deals, by day, week or month depending on
  the period's length. Hovering shows a period's figures, and there is a table view.
- **Funnel:** for deals created in the period, how many reached each stage, the conversion
  from one stage to the next, and how many were won and lost. It works from the stage history
  recorded on every stage change; a won deal counts as having passed every stage. History from
  before this feature is rebuilt from the timeline.
- **Managers:** amount won, deals won, win rate, new and open deals, completed activities, and
  progress against this month's target.
- **Sales targets:** CRM admins set monthly targets for the team and for each person in the
  base currency (**Set targets**). Progress counts deals won in that month.
- **Sources** and **lead conversion** (converted, still open, junk) for records created in the
  period.
- **Gross profit:** for records won in the period, net sales (after discounts, without tax) of
  their product rows, cost, gross profit and margin — in total and by manager, product, month
  and record. Rows without a known cost are left out of profit and margin and shown apart, as
  are records won without product rows.
- **Rules:**
  - Amounts are in the base currency.
  - Days follow the viewer's time zone.
  - Won and lost count by closing date; new deals and the funnel count by creation date.
  - Everything is limited to records the viewer may see.


#### True profit

CRM → Analytics → **True profit** (for those who read the books, and CRM administrators): what
each client, deal, product or salesperson really earned in a period.

- **Revenue:** invoices in the period (not lost or deleted), without tax and after discounts
  (the discounts are shown); in the base currency.
- **Goods:** what the products cost — each row's cost, which follows the stock's FIFO or
  average cost for counted products, else the product's cost.
- **Support time:** minutes support logged answering the client's tickets (a company's
  contacts count too), and **task time:** time logged on tasks linked to the client, its
  contacts, deals and invoices — each at the person's hourly cost (Costs and rates: per person,
  or a default). A client's time is spread over its invoices by revenue.
- **Late payment:** for invoices in the books, days paid after the due date (or overdue so
  far) × the amount × the yearly financing rate.
- Profit = revenue − goods − support − task time − late payment, with the margin. The same
  invoice figures add up by client, deal (through a quote to its deal), salesperson (the
  invoice's responsible person) or product (its costs spread over its rows by revenue).
- Hourly costs and rates are set by those who manage employees or CRM administrators.

#### Insights: deal health, next steps, silent clients

CRM → **Insights**, built only from the company's own records (no outside service).

- **Deal health:** each open deal scores from 100 down against how the same pipeline's won
  deals went, with the reasons: no contact for a while, slower than won deals usually move
  through the stage, fewer people involved than won deals usually have, our email unanswered
  for over 5 days, the close date passed (or none set), nothing planned next, and a client who
  pays late. 70 and up is healthy, 40–69 at risk, below that poor. The list puts the least
  healthy first, by pipeline, grade or "mine"; the deal page shows its own health card.
- **What worked at this stage:** on the deal's card, the steps (call, meeting, to-do, email,
  document) that won deals took in the current stage and lost ones mostly didn't — with how
  often deals won with and without it, and the usual day it happened; ticked once taken.
- **Silent clients:** clients with at least three orders (won deals or completed shop orders)
  who are well past their usual gap (over 1.5× the median gap and at least two weeks over it).
  CRM administrators can turn on a daily check-in task for the client's responsible person,
  made once each time a client goes quiet.
- Everything is limited to records the viewer may see.

### Automation (robots and triggers)

CRM settings → Automation shows each pipeline as columns of stages.

- **Tunnels:** the "Tunnel to another pipeline" robot copies the record (linked, with its
  client, amount, products and custom fields) or moves it into a stage of another pipeline
  of the same type. Pipelines & stages lists the tunnels as arrows.

- **Robots** belong to a stage and act only if their conditions hold (field = ≠ > < ≥ ≤,
  contains, empty). For dates, compare with `today`, `today+7`, `today-3` or `now` (UTC).
  - **When they run:**
    - when the record arrives in the stage;
    - after a delay;
    - or relative to one of the record's dates, for example "2 days before the expected close
      date" or "1 hour after a custom date-time field". A date without a time counts from 09:00
      in the responsible person's time zone. Changing the date reschedules the robot, clearing
      it cancels the robot, and a time already in the past runs at once.
  - **In sequence:** "Start only after the robot above it has finished" chains robots. The ↑ ↓
    buttons on a robot card set the order.
  - **Waiting:** a robot can wait until conditions hold (checked whenever the record changes),
    until the client replies by email, or until an activity is completed, for at most a set
    time. After that time it either runs anyway or is skipped.
  - Robots waiting in a stage the record has left are skipped at once.
  - **What they can do:**
    - plan an activity, create a task or notify employees;
    - email the client, logged as an email activity, optionally with a document attached: the
      PDF is generated first and the email goes out when it is ready;
    - change a field;
    - change the responsible person: in turn, at random, or to whoever has the fewest open
      records;
    - move to another stage, create a document or add a comment;
    - call a webhook;
    - create a linked record: a lead, deal or smart-process record, optionally with the same
      company, contacts and amount. The new record shows where it came from;
    - convert a lead into a contact, company and/or deal.
  - Texts can use `{{title}}`, `{{amount}}`, `{{stage}}`, `{{assigned}}`, `{{company}}`,
    `{{contact}}`, `{{client_email}}`, `{{link}}` and `{{uf.<code>}}`.
- **Outgoing webhooks:**
  - GET, POST, PUT, PATCH or DELETE to any http(s) URL, which may contain placeholders. Up to
    10 headers can be added.
  - The body is either a template or, when empty, the whole record as JSON. A template that
    starts with `{` or `[` is sent as JSON, with placeholder values escaped.
  - With a signing secret, each call carries `X-NERP-Signature: sha256=<HMAC-SHA256 of the
    body>`, plus `X-NERP-Event` and `X-NERP-Record` headers.
  - Calls time out after 15 s and redirects are not followed.
  - Server errors (5xx, 429) and network failures are retried after 1, 5 and 30 minutes;
    other 4xx responses fail at once.
  - Addresses in private networks (loopback, LAN, link-local, cloud metadata) are refused
    unless `CRM_WEBHOOK_ALLOW_PRIVATE=true`. The name is resolved once and the connection is
    pinned to the checked address.
- **Stage time limits:** each open stage can have a limit, set with "+ Time limit" in its
  column. A record that stays longer gets a red "over the limit" mark on the board and its
  page, and an entry in its timeline. Its responsible person is notified; with escalation, the
  heads of their departments are notified too (for a head, the head of the parent department).
- **Schedules** (the Schedules tab) run an action daily, weekly (on chosen days) or monthly (on
  a day; past the month's end means its last day), at a time in a chosen time zone. Each run
  covers every record of the pipeline that is in the chosen stages (or every open stage) and
  matches the conditions, up to 5,000 records. A typical schedule: "every Monday at 09:00,
  move records whose expected close date is before `today` to Follow-up". "Run now" starts a
  schedule immediately, without changing its next run.
- **Triggers** move a record to their stage when an activity is completed (optionally of one
  kind), a comment is added, a document is created (optionally from one template), or an
  external system calls the trigger's secret webhook URL (`POST {"item_id": …}`, no login,
  rate-limited). By default they only move records forward.
  "All its tasks are done" moves a record on once every task linked to it (for instance the
  ones its stage's robots handed out) is done, so a smart process can run as a simple RPA
  flow: each stage gives people their tasks, and the record moves on by itself when they
  finish.
- **How it runs:** robots run in the background job queue as the built-in "Automation" user.
  That user is hidden from employee lists and cannot sign in. Every run is recorded, and done
  or failed runs appear in the record's timeline. Waiting robots are listed on the record.
- **Test mode** (per pipeline, CRM admins): while it's on, the pipeline's robots, triggers and
  schedules run only for records whose responsible person is one of the chosen testers;
  everyone else's records are left alone. Runs made in test mode are marked "test" in the log
  and on the record.
- **Log** tab: every robot and schedule run of the pipeline in a period, with counts per status
  and per robot or schedule. You can search by record, robot or result, and retry a failed run
  (it checks the stage and conditions again but does not wait again).
- **Time in stages** tab: for each open stage, the median, average and longest time records
  stayed, how many entered, how many are there now, and how many are over the limit now and
  went over it in the period.
- **Loop protection:** chains of automated stage changes stop after 5 steps, and each record
  has a budget of 200 robot runs per hour. When either limit is hit, the timeline says so.

### Record cards

Each record type's page shows its fields in sections (by default "About", "Contact details"
for people and companies, and "Comment"). The record's form uses the same sections. The
identity fields stay at the top and are not part of the card: name, amount, pipeline and
stage, company and contacts, and responsible person.

- **Customize** (on the first section) opens the layout editor. In it you can:
  - rename, add, reorder and remove sections; a removed section's fields move to the one
    above it;
  - reorder fields and move them between sections;
  - hide fields. A hidden field keeps its value but isn't shown or edited on the card.
    Required custom fields can't be hidden;
  - choose to leave out empty fields on the record page.
- **Who sees it:** "Just for me" saves a personal layout. CRM admins can also save "For
  everyone". A personal layout wins over the shared one, and "Use the shared card" drops it.
  Admins can reset the shared card to the built-in default.
- **Fields added later:** a new custom field appears at the end of the last section, and a
  deleted one drops out of the layout.
- A new record's form shows the first section, with the others behind "More fields".
- API: `GET`, `PUT {layout, scope: "mine" | "common"}` and `DELETE ?scope=` on
  `/api/crm/{type}/card-layout`.

### Working together on records

- **Edit history:** each save adds one timeline entry listing what changed, with old → new
  values. Tracked: names and title, amount and currency, dates, source, company, contacts,
  phone/email/website, address, comment, pipeline and custom fields. Changes made by
  automation, bulk edits and imports into existing records count too. Stage moves and new
  responsible people have their own entries.
- **Observers:** anyone who can edit a record can add employees as observers (up to 50).
  Observers:
  - can open the record, its timeline and products, comment and plan activities, even
    outside their usual access scope; they can't edit it;
  - see it in their lists;
  - are told when it moves stage or gets a comment (the responsible person is told about
    comments too).

  The robot action "Add observers" does the same automatically.
- **Tasks:** a record's "Tasks" card lists the tasks linked to it that the viewer may see, and
  "+ Task" creates one already linked. On a task, the "CRM" card links and unlinks records:
  deals, leads, contacts, companies, quotes and invoices. Creating, linking and completing a
  linked task is noted in the record's timeline. Tasks made by robots are linked to their
  record.
  - API: `crm_item_ids` when creating a task; `PUT /api/tasks/{id}/crm {item_ids}`.
- **Email templates:** in the email composer, "Use a template…" fills in the subject and
  message for the current record. The same placeholders as robots work, plus `{{me.name}}`
  and `{{me.email}}` for the sender. "Save as template" keeps the current email as a personal
  template; CRM admins can share it with everyone. A template saved from a record type is
  offered for that type only.
  - API: `/api/crm/email-templates` (list, create, update, delete) and
    `POST /api/crm/email-templates/{id}/render {item_id}`.

### Quotes and invoices

Quotes and Invoices are built-in record types in the sidebar. They work like smart
processes: stages, products with tax, a client, document templates, automation, analytics
and access rights. Admins can rename them and add pipelines or fields, but cannot delete them.

- **Stages:** quotes go Draft → Sent → Accepted / Declined; invoices go Draft → Sent →
  Paid / Cancelled. A paid invoice records when it was paid.
- **Creating them:** "Create quote" and "Create invoice" on a deal, and "Create invoice" on a quote or an
  order. Each copies the title, company, contacts, responsible person, currency, amount and
  product rows, and links back to its source ("Created from …"). A quote is valid for 30 days
  and an invoice is due in 14, by default.
  - API: `POST /api/crm/{quote|invoice}/items/from/{source id}`.
- **On the deal:** a "Quotes and invoices" card lists the deal's quotes, their invoices and its
  own invoices, with stage, due date and amount, plus the totals invoiced and paid in the
  deal's currency.
- **Overdue:** an unpaid invoice past its due date is marked "Overdue" on the board, on its
  page and on the deal. To chase overdue invoices automatically, use a robot timed from the
  due date or a schedule with the condition "Due date < `today`".
- The built-in invoice and quote document templates are offered for these types.

### Recurring records

A deal, invoice, quote, order or smart-process record can repeat: the "Repeat" card on its
page sets a schedule (daily, weekly or monthly at a local time, from a start date, with an
optional end date or number of copies), the same rules as recurring tasks.

- Each time the schedule comes round, a copy of the record as it is then starts in the first
  stage of its pipeline: title, responsible person, company and contacts, currency, amount,
  custom fields, comment and products. Its date (close date, due date…) can be set a number
  of days ahead. The first stage's robots run as usual, e.g. to email a new invoice.
- Copies are listed under the record's "Made from this record", and both get timeline
  entries. "Create one now" makes an extra copy without moving the schedule.
- A responsible person who has left is not copied; the copy waits for someone new. Records
  in the recycle bin don't repeat. If a copy can't be made, whoever set the schedule is
  notified.

### Product catalog: properties, variants, price types

Set up under Products → Catalog settings by people who manage the catalog.

- **Section editors:** each category (Products → Shop categories → Editors) can name people
  and departments who add, change and remove its products without the catalog-wide
  permission.

- **Properties:** details products can have, as text, numbers or a list of values
  (Material, Weight…). Each product form shows them; the shop shows the ones marked
  "shown in the shop" on the product page.
- **Variants:** a list property marked "makes variants" (Size, Colour…) lets a product be
  sold in versions. "Make variants" on the product creates one product per combination of
  the values picked (existing combinations are skipped), copying its price, tax, unit,
  description, property values and shop settings. Each variant then has its own name, SKU,
  price, stock and photo (it shows its parent's photo and description until it has its
  own), and can be put on deals and stock documents like any product. The product list
  shows variants under their parent. Deleting a product whose variants were sold deactivates
  it with its variants instead.
- **In the shop**, a product with variants is listed once, "from" its lowest price; its page
  asks for each choice (combinations that don't exist are greyed out) and shows the chosen
  variant's price and stock. Only variants can be ordered.
- **Price types:** extra prices beside the base price (Wholesale, Partner…), set on each
  product form. When adding products to a deal, quote or invoice, "Prices" picks which price
  rows get; changing it re-prices the catalog rows already there.

### E-signature

Any ready document generated from a record (quote, invoice, contract…) can be sent for
signature from the record's Documents card, by anyone who can edit the record.

- **Sending:** up to ten signers (name and email) and an optional message. The PDF is frozen
  at that moment (regenerating the document later doesn't change what is being signed) and
  its SHA-256 fingerprint recorded. Each signer gets a personal link by email, valid for 30
  days. One request per document at a time.
- **Signing page** (no sign-in): the PDF, the fingerprint, who else is signing; the signer
  types their name, draws their signature (mouse, finger or pen) and agrees to sign
  electronically, or declines with an optional reason. The time, IP address and browser are
  kept with each signature.
- **When everyone has signed**, a signed copy is made: the document followed by a
  certificate page listing each signer, their signature, the name they typed, when, from
  where, and the document's fingerprint. Every signer gets it by email; on the record it is
  under the document ("Download the signed copy").
- **The sender** sees each signer's progress (not opened, opened, signed, declined), is
  notified of each signature or refusal, can send the links again (new links; old ones stop
  working) or cancel the request. All of it goes on the record's timeline.
- Signed copies, frozen PDFs and signature images are deleted with the record when it is
  emptied from the recycle bin; merging records keeps them.
- **Signing templates** (CRM settings → Documents) name the signers by role: the record's
  contact, its company, the record itself, the responsible person, whoever sends it, or a
  fixed person; with a message and optionally a document template to make the document
  from. Choosing one in the send dialog fills the signers in for that record.
- **Automation:** triggers on "Everyone signs a document" and "Someone declines to sign"
  (optionally for one signing template), and a "Send for signature" robot that sends in the
  name of the record's responsible person (a document still rendering goes out when ready).
- **Who sends:** a company setting limits e-signature to people with "Send for
  e-signature" (and CRM admins).

### Smart processes (your own record types)

CRM settings → Record types creates record types such as Contracts, Shipments or Service
requests.

- **Same engine as deals:** each type runs on the same item engine and gets everything deals
  have: a Kanban board and list, stages (New, In progress, Done, Cancelled to start with),
  custom fields, filters, import/export, access rights per role (copied from deals at first),
  automation, document templates, timeline, activities and the recycle bin.
- **Options:**
  - linked to a company and contacts (shown on their pages; documents are addressed to them)
  - an amount with product rows
  - several pipelines
- **Where they appear:** in the sidebar under CRM, at `/crm/<code>`. The code comes from the
  plural name and never changes after a rename.
- **Deleting:** a type can only be deleted once it has no records, including the recycle bin.

### Email

People connect their own mailboxes under CRM → Mailboxes: any IMAP + SMTP account (Fastmail,
Zoho, iCloud, Yahoo, or your own server; there are presets for the big providers). A
connection test runs before saving.

- **Receiving:** NERP checks each mailbox every minute, plus on demand, reading the inbox and
  the sent folder.
  - A message is filed when it continues a conversation already in the CRM, or when another
    party's address matches a lead, contact or company. New conversations with a known
    contact also go on their latest open deal (or smart-process record).
  - Unknown senders can become leads if the mailbox is set to do that.
  - Everything else is ignored, so private mail never enters the CRM.
  - The first sync files the last N days (7 by default).
- **On the record:** each email is a timeline entry you can open, with an open "email" activity
  on the main record until it's answered.
  - HTML is sanitised on the server and shown in a sandboxed frame that runs no scripts.
  - Remote images (tracking pixels) stay blocked until you choose "Show images".
  - Attachments are stored as files that anyone who can read the record can download.
- **Writing:** the Email tab on a record suggests the record's, its contacts' and its
  company's addresses.
  - You can attach files or the record's generated documents.
  - Replies keep the conversation's `In-Reply-To` and `References` headers and quote the
    previous message.
  - Sending runs in the background through the mailbox's SMTP server, and a copy is kept in
    its Sent folder. A failed send is shown on the record and can be retried.
- **From an email:** "Make a task" (its subject the title, its text the description, linked
  to the email's records), "Plan an event" (on the record), and "Discuss" — post it, with a
  note of your own and a link to its record, in a chat you're in or in the news feed.
- **Automation:** the "Email the client" robot sends from the responsible person's mailbox when
  they have one. A new trigger fires when an email arrives from the client.
- **Sharing:** a mailbox can be shared with chosen people, roles and departments (with
  sub-departments); they read its mail and send from it.
- **Admin → Mailboxes** ("Manage everyone's mailboxes"): every employee's connected mailbox
  with its status and last sync. Admins connect many employees' IMAP/SMTP mailboxes at once
  (one row each: employee, address, password), sync them, change their sharing or remove
  them. Each employee is told when an admin connects or removes their mailbox, and the
  passwords are stored encrypted like any other.
- **Security:**
  - Passwords are encrypted with AES-256-GCM. The key comes from `DATA_ENCRYPTION_KEY` (32+
    characters), or is derived from `JWT_SECRET` when that is unset.
  - Changing the key means people re-enter their mailbox passwords.
  - Gmail and Microsoft 365 sign in with OAuth (below); no password is ever stored for them.

#### Gmail and Microsoft 365 (OAuth)

These providers no longer accept passwords over IMAP/SMTP. Once an administrator registers an
OAuth app, users get "Connect Gmail" / "Connect Microsoft 365" buttons.

- **How it works:** sign-in uses the authorisation-code flow with PKCE. The refresh token is
  stored encrypted, short-lived access tokens are refreshed automatically, and IMAP and SMTP
  sign in with XOAUTH2.
- **Reconnect:** if access is revoked, the mailbox shows the error and a "Reconnect" button.
- **Redirect URI:** Google and Microsoft both use `{PUBLIC_URL}/api/crm/mailboxes/oauth/callback`,
  built from `PUBLIC_URL`. The System page shows the exact value the server uses.

  | Setup | `PUBLIC_URL` | Redirect URI to register |
  | --- | --- | --- |
  | Development with the Vite dev server (`npm run dev`, which forwards `/api`) | `http://localhost:5173` | `http://localhost:5173/api/crm/mailboxes/oauth/callback` |
  | Development, app served by the backend (`STATIC_DIR`) | `http://localhost:8080` | `http://localhost:8080/api/crm/mailboxes/oauth/callback` |
  | Production behind your TLS proxy | `https://crm.example.com` | `https://crm.example.com/api/crm/mailboxes/oauth/callback` |

  - The URI must match exactly: scheme, host, port and path, with no trailing slash.
  - Plain `http://` is accepted only for `localhost`; anything else needs `https://`.
  - Both providers allow several redirect URIs on one app, so you can register the
    development and production ones together.
  - Where to enter it:
    - **Google:** Cloud Console → *APIs & Services → Credentials* → your OAuth client (type
      *Web application*) → *Authorized redirect URIs*.
    - **Microsoft:** Entra admin center → *App registrations* → your app → *Authentication* →
      *Add a platform* → *Web* → *Redirect URIs*.
  - If `PUBLIC_URL` changes, update the registered URIs too. Otherwise the provider stops the
    sign-in with a "redirect_uri mismatch" error.

**Google** (Google Cloud Console):
1. Create a project, then under *APIs & Services → OAuth consent screen* choose *Internal*
   (Workspace users only) or *External*.
2. Add the scope `https://mail.google.com/`, plus `openid`, `email` and `profile`.
3. Under *Credentials*, create an *OAuth client ID* of type *Web application* and add the
   redirect URI.
4. Set `GOOGLE_CLIENT_ID` and `GOOGLE_CLIENT_SECRET`.
5. Note: an *External* app with the `mail.google.com` scope must pass Google's verification
   before people outside your test-user list can connect. *Internal* apps don't need it.

**Microsoft 365 / Outlook.com** (Microsoft Entra admin center → App registrations):
1. Register a new app. Choose the supported account types, and add a *Web* platform with the
   redirect URI.
2. Under *Certificates & secrets*, create a client secret.
3. Under *API permissions*, add delegated *Office 365 Exchange Online* (shown as
   `outlook.office.com`) permissions `IMAP.AccessAsUser.All` and `SMTP.Send`, plus
   `offline_access`, `openid`, `email` and `profile`. Grant admin consent if your tenant
   requires it.
4. Set `MICROSOFT_CLIENT_ID`, `MICROSOFT_CLIENT_SECRET`, and `MICROSOFT_TENANT`: `common`
   (default, any account), `organizations`, or your tenant id.
5. In Exchange, *Authenticated SMTP* must be enabled for the mailboxes that send.

### Web forms

CRM → Web forms builds forms for your website. CRM admins manage them.

- **Builder:**
  - Fields can be text, long text, email, phone, number, date, choice or checkbox, each
    required or optional.
  - Each answer is saved into a record field, or into the record's comment.
  - You set the heading, introduction, button, thank-you message or redirect, colour, and an
    optional consent checkbox.
  - A live preview shows the form as you build it.
- **What a submission creates:**
  - A lead.
  - Or a deal or smart-process record with its contact (matched by email or phone, otherwise
    created) and company (matched by name, otherwise created), in a chosen pipeline and stage,
    with an optional amount.
  - Responsible people are assigned in turn and notified.
  - The record gets source "Web form", the page address, the campaign tags (`utm_source`,
    `utm_medium`, `utm_campaign`, `utm_content`, `utm_term`), and a timeline entry with all
    the answers.
  - An optional automatic reply goes out from a connected mailbox, and the trigger "A web
    form is submitted" fires.
- **Publishing:**
  - A hosted page at `/f/<key>`.
  - Or an embed: `<script src="{PUBLIC_URL}/embed.js" data-form="<key>" async></script>`. It
    inserts the form in an iframe that resizes itself and passes the host page's address, so
    campaign tags from your landing pages are recorded.
  - A plain iframe also works.
- **Spam protection:**
  - a hidden decoy field (bots get a fake "thank you" and nothing is stored)
  - a signed "form shown at" token with a minimum fill time
  - rate limits of 10 submissions per visitor per 10 minutes and 2,000 per form per hour
  - server-side checks of every value
- **Submissions log:** each form has one, with links to the created records. Deleting a form
  keeps its records.

Next CRM phases: analytics, then telephony and marketing.

Other candidates: Chat, Calendar, Drive, Workflows.

## Layout

```
backend/
  migrations/          SQL migrations (run automatically at start-up)
  src/main.rs          start-up: config, database creation, migrations, router, static SPA
  src/auth/            passwords (argon2), JWT and refresh tokens, permission catalog, extractors
  src/routes/          one file per API area
frontend/
  src/lib/             API client, session store, shared components
  src/routes/(public)  login, setup
  src/routes/(app)     the signed-in application
tests/e2e/             API and browser end-to-end suites (see tests/e2e/README.md)
tests/db/              database feature and migration checks
```

## Development

Prerequisites: Rust 1.85+, Node 22+, PostgreSQL 18 (with the bundled `pgcrypto` and `pg_trgm`
extensions), and Redis or Valkey. The database user needs to create the `nerp` database on first
start (or create it yourself and grant the user ownership).

```sh
# backend: creates the `nerp` database on first run and applies migrations
cd backend
cp .env.example .env      # set DATABASE_URL, JWT_SECRET; COOKIE_SECURE=false for http://localhost
cargo run                 # http://127.0.0.1:8080

# frontend: proxies /api to the backend
cd frontend
npm install
npm run dev               # http://localhost:5173
npm run check             # type checking
```

Open the app. The first visit shows the setup screen.

## Tests

```sh
# backend unit tests
cd backend && cargo test

# frontend type checks
cd frontend && npm run check

# end-to-end suites (TypeScript, run directly by Node), against a running server on http://localhost:8080
cd tests && npm ci
npx playwright install chromium-headless-shell
npm run check                              # type-check the suites
node e2e/run-all.ts                        # everything (about 10 minutes)
node e2e/run-all.ts api                    # API suites only (or: ui)
node e2e/run-all.ts api15 ui15             # chosen suites
node e2e/api18.ts                          # one suite, every check printed

# remove what interrupted runs left behind (shows it first; --yes removes it)
node tests/db/cleanup-test-data.ts
```

Each suite removes the data it created. What the end-to-end suites need (mail servers,
LaTeX/LibreOffice, server settings) is listed in `tests/e2e/README.md`.

## Production (bare metal)

```sh
cd frontend && npm ci && npm run build          # -> frontend/build
cd backend  && cargo build --release            # -> backend/target/release/nerp
```

Run the binary with `STATIC_DIR` pointing at `frontend/build`. One process then serves both the API
(`/api/*`) and the app. Put it behind a TLS-terminating reverse proxy, keep `COOKIE_SECURE=true`, and
set `TRUST_PROXY=true` only if that proxy sets `X-Forwarded-For`.

Compression and caching are built in. The frontend build ships brotli and gzip copies of every
file, served by `Accept-Encoding`. Hashed files under `/_app/immutable/` are cached for a year,
while `index.html` is revalidated so deploys show up at once. API responses (JSON, CSV) are
compressed on the fly; PDFs, images and office files are sent as they are. The proxy can pass
responses through unchanged and must keep the `Cache-Control` headers.

For documents in every script, install the Noto fonts (e.g. `noto-fonts noto-fonts-cjk` on Arch,
`fonts-noto fonts-noto-cjk` on Debian/Ubuntu). Optional engines: TeX Live with LuaLaTeX, fontspec,
babel, longtable, booktabs, fancyhdr and lastpage (e.g. `texlive-luatex texlive-latexextra`), and
LibreOffice (`libreoffice-fresh` / `libreoffice-writer`). LibreOffice needs no display server.

## Configuration

All settings are environment variables (see `backend/.env.example`):

| Variable | Default | Notes |
|---|---|---|
| `DATABASE_URL` | — | `postgres://user:pass@host:5433/nerp` |
| `DATABASE_MAX_CONNECTIONS` | `20` | connections in the pool |
| `REDIS_URL` | `redis://127.0.0.1:6379` | rate limits, lockouts, caches and live updates; turn on its append-only file (SETUP.md). Sessions and emailed links are in PostgreSQL |
| `JWT_SECRET` | — | at least 32 characters |
| `BIND_ADDR` | `127.0.0.1:8080` | |
| `UPLOAD_DIR` | `./data/uploads` | back this up together with the database |
| `MAX_UPLOAD_BYTES` | 50 MB | |
| `STATIC_DIR` | empty | directory with the built frontend |
| `ACCESS_TOKEN_TTL_SECS` / `REFRESH_TOKEN_TTL_SECS` | 900 / 2592000 | |
| `COOKIE_SECURE` | `true` | |
| `TRUST_PROXY` | `false` | |
| `CORS_ORIGINS` | empty | only if the frontend runs on another origin |
| `PUBLIC_URL` | `http://localhost:5173` | base URL used for links in emails |
| `SMTP_HOST` / `SMTP_PORT` | `127.0.0.1` / `1025` | |
| `SMTP_SECURITY` | `none` | `none`, `starttls` or `tls` |
| `SMTP_USERNAME` / `SMTP_PASSWORD` | empty | |
| `SMTP_ACCEPT_INVALID_CERTS` | `false` | `true` only for local servers with self-signed certificates (e.g. Mailpit) |
| `MAIL_FROM` | `NERP <no-reply@nerp.local>` | quote it in `.env` (`MAIL_FROM="NERP <no-reply@…>"`): an unquoted value with spaces stops `.env` at that line, and the server logs a warning |
| `WORKER_ENABLED` / `WORKER_CONCURRENCY` | `true` / `4` | run the job worker in at least one instance |
| `LUALATEX_BIN` / `SOFFICE_BIN` | `lualatex` / `soffice` | external document engines (optional; Typst is built in) |
| `DOC_RENDER_TIMEOUT_SECS` / `DOC_RENDER_CONCURRENCY` | `60` / `2` | limits for LuaLaTeX and LibreOffice runs |
| `CRM_WEBHOOK_ALLOW_PRIVATE` | `false` | let automation webhooks call private-network addresses (loopback, LAN) |
| `COLLABORA_URL` | unset | Collabora Online for editing office files in the browser, e.g. `http://127.0.0.1:9980`. Run the official `collabora/code` image (see `tests/e2e/tools/collabora-up.sh`) with NERP's address in its `aliasgroup1`; without it files still get previews through LibreOffice |
| `MAIL_ALLOW_PRIVATE` | `false` | let mailboxes use mail servers on private networks (loopback, LAN), e.g. a company mail server |
| `DATA_ENCRYPTION_KEY` | derived from `JWT_SECRET` | key for secrets stored in the database (mailbox passwords); 32+ characters |
| `FILE_ENCRYPTION_KEY` | unset | encrypts uploaded files at rest (AES-256-GCM); 32+ characters. Existing files are encrypted in the background; keep the key with `.env`'s backup — without it the files can't be read. The server refuses to start when files are under a key it wasn't given |
| `FILE_ENCRYPTION_OLD_KEYS` | empty | earlier file keys (comma-separated) while files move to a new `FILE_ENCRYPTION_KEY`; with no current key, files are decrypted back |
| `FILE_KEEP_DELETED_DAYS` | `14` | removed files are kept aside in `UPLOAD_DIR/.deleted` this long, for point-in-time restores (`0`: removed at once) |
| `WAL_ARCHIVE_DIR` | `<BACKUP_DIR>/wal` | where `nerp-wal-receive.sh` keeps the database's WAL for point-in-time restores (read by the backup and restore scripts) |
| `WAL_SLOT` | `nerp_wal` | the replication slot `nerp-wal-receive.sh` streams through; System → Backups watches it |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` | — | enables "Connect Gmail" (see Email → OAuth) |
| `MICROSOFT_CLIENT_ID` / `MICROSOFT_CLIENT_SECRET` / `MICROSOFT_TENANT` | — / — / `common` | enables "Connect Microsoft 365" |
| `PDF_FONT_DIR` | — | extra fonts for Typst, besides the system fonts |
| `VIDEO_CONVERT_MAX_MB` | `2048` | larger videos aren't converted into a copy every browser plays |
| `GOOGLE_OAUTH_AUTH_URL` / `GOOGLE_OAUTH_TOKEN_URL`, `MICROSOFT_OAUTH_AUTH_URL` / `MICROSOFT_OAUTH_TOKEN_URL` | the providers' own | tests or proxies only: other addresses for the OAuth endpoints |
| `GOOGLE_CALDAV_URL` / `MICROSOFT_GRAPH_URL` | the providers' own | tests or proxies only: other addresses for calendar sync |
| `LDAP_ENABLED` | `false` | `true` offers LDAP directory sign-in and sync (set up under Directory & sign-on) |
| `LIVEKIT_URL` | empty | a LiveKit media server for calls (`ws://`/`wss://`, what browsers reach); empty: direct calls |
| `LIVEKIT_API_URL` | from `LIVEKIT_URL` | where NERP reaches LiveKit's API (`http://`/`https://`) |
| `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` | — | required with `LIVEKIT_URL` |
| `LIVEKIT_RECORDINGS_DIR` | empty | folder LiveKit Egress writes recordings to (as NERP sees it); empty: no recording |
| `LIVEKIT_EGRESS_DIR` | `LIVEKIT_RECORDINGS_DIR` | the same folder as Egress sees it, if different (e.g. in a container) |
| `BACKUP_RESTORE` | `false` | `true` lets System → Backups restore a backup over the live data (with the person's password and the backup's name typed; the app pauses meanwhile). Off: the page shows the command instead |
| `BACKUP_DIR` | `/var/backups/nerp` | where `deploy/backup/nerp-backup.sh` writes backups; the System → Backups page lists them and runs the scripts |
| `BACKUP_SCRIPTS` | `../deploy/backup` | the folder holding the backup scripts, as the server sees it |
| `NERP_TEST_SIGNIN_BASE` | empty | tests only: send every outside sign-in provider (Google, Microsoft, Apple, Facebook, X) to a stand-in at this address; never set it in production |

## Database notes

- NERP runs on PostgreSQL 18. It used YugabyteDB until November 2026; to move such a database,
  start NERP once on an empty PostgreSQL database (the migrations create the schema), stop it,
  empty the rows the migrations seeded, and load a data-only dump made with `ysql_dump
  --data-only --disable-triggers` (leaving out `_sqlx_migrations` and `_nerp_migration_progress`),
  then compare the row counts table by table.
- Migrations (`backend/src/migrate.rs`) run each file in one transaction, so a failed migration
  leaves nothing half done. An advisory lock lets several instances start at once: one migrates,
  the others wait.
- Primary keys are UUIDs. Human-readable numbers (tasks, documents, invoices) come from counter
  rows updated in the creating transaction, so they have no gaps.

## Marketing

Shown in the sidebar to people with the "Run marketing" permission.

- **Segments:** saved audiences of leads, contacts or companies. Filters: name/email/phone
  search, source, lead status, created dates, responsible person. The editor counts, as you
  type, how many records match, how many have an email address, how many unsubscribed, and
  so how many can be reached; it also shows the first few. Segments are worked out again at
  each send, so new records join automatically.
- **Campaigns:**
  - **Writing:** a draft has a sender name, a reply-to address, a subject and a plain-text
    message with placeholders (`{{first_name}}`, `{{title}}`, `{{company}}`, …). Send a test
    to any address, filled in from the first person in the audience.
  - **Designed emails:** instead of plain text, a message can be built from blocks in the
    email designer — heading, text (with **bold**, *italic* and `[label](https://…)`
    links), button, image (uploaded or by address), two columns, divider, space and social
    links — dragged in from the palette, reordered by dragging and set up on the side, with
    a page style (colours, font, the preview text inboxes show). The preview in the middle is
    rendered by the server exactly as it is sent: table-based HTML with inline styles, which
    mail programs understand, and a plain-text version alongside. Placeholders are filled per
    recipient and escaped, links go through click tracking, and uploaded images are served
    from a signed public address (mail programs load images without signing in).
  - **Sending:** send now or schedule it. Sending starts by listing one recipient per email
    address across the chosen segments, leaving out unsubscribed addresses and records with
    none. Emails then go out in the background, 300 a minute, through the configured SMTP
    server. A scheduled campaign can be cancelled (back to a draft); one being sent can be
    stopped.
  - **What each email carries:**
    - a footer with an unsubscribe link;
    - `List-Unsubscribe` and `List-Unsubscribe-Post` headers, so mail clients show
      "Unsubscribe" (one click);
    - an open pixel;
    - signed click-tracking links, which also get `utm_source=nerp&utm_medium=email&utm_campaign=<campaign>`.
  - **Results:** recipients, sent, opened, clicked, unsubscribed and failed, with rates;
    clicks per link; and what the campaign brought in (leads and deals whose campaign tag
    matches, e.g. from web forms reached through its links, with won amounts). The
    recipient list can be filtered by opened, clicked, unsubscribed or failed. Each
    recipient's record gets a timeline entry when it is sent the campaign and when they
    unsubscribe.
- **Sales generator** (`/marketing/generators`): a deal or lead for every client in a segment
  of contacts or companies — repeat sales, renewals, seasonal offers — now, at a set time, or
  every week, month or quarter. Records go to the client's responsible person or to chosen
  people in turn, in the chosen pipeline and stage, titled from a template (`{{name}}`,
  `{{month}}`, `{{year}}`), linked to the client. Clients who already have an open record of
  that kind (in that pipeline) can be skipped. Running one by hand first shows how many it
  will make and skip, and keeps its schedule. The generator lists what it made, and its
  author is told after each run.
- **Traffic** (`/marketing/traffic`), where clients come from:
  - **Websites:** a tracking script for each company website
    (`<script src="…/track.js" data-site="KEY" async>`), optionally limited to its host names.
    It keeps a visitor id in the browser and reports each page, single-page sites included.
    Pages within 30 minutes of each other from one source make a visit, put down to a
    channel — paid ads (click ids or paid mediums), search engines, social networks, email,
    other websites, tagged campaigns, offline ads or direct — and a source (a CRM traffic
    source when its UTM tag matches). Browsers asking not to be tracked (Global Privacy
    Control, Do Not Track) are left alone. Web forms and the live chat on those pages pass
    the visitor id along, so the CRM record they make carries the visitor's **journey**: a
    card on the record (and, for a deal, its contacts' and company's) with every visit,
    where it came from and the pages seen, then the record's own milestones (made, stages,
    won or lost).
  - **Analysis:** for a period and website, visits, visitors and new visitors by channel or
    by source, and what visitors who first came that way became: CRM records, deals opened,
    deals won and revenue, against the spend entered — conversion, cost per record and ROI.
  - **Ad links:** short links (`/go/<code>`) and QR codes for posters, print, radio or events.
    They send people on to a page with UTM tags and count clicks (once an hour per address);
    visits through them count as offline ads for the link's source.
  - **Spend:** what each channel or source cost per month, in any enabled currency
    (converted to the base currency when entered).
  - Page lists are kept for 400 days, and visitors who never became a CRM record are
    forgotten after 400 days without a visit.
- **Unsubscribes:** the public unsubscribe page shows the address masked and asks once.
  The list shows who unsubscribed from which campaign and lets you add addresses or remove
  one (for someone who asked to get emails again). The list only affects campaigns; emails
  about someone's own deals, tasks or documents still go out.

## Online shop

An online shop on top of the product catalog, at `PUBLIC_URL/shop`, with no sign-in. It is
set up under CRM → Online shop by people with the "Run the online shop" permission.

- **Settings:** open or close the shop, its name and introduction, the currency it sells in,
  who handles orders, how customers pay, and the terms of sale. The shop cannot open without
  payment instructions. In those, `{{order}}` becomes the order number and `{{total}}` the
  amount. While the shop is closed, its pages say so, but customers' order links still work.
- **What it sells:** on the product form, "Sell in the online shop", a category (managed
  under Products → Shop categories) and a photo. A product gets its shop address the first
  time it is listed and keeps it. Only active products priced in the shop's currency are
  listed; the settings page counts the ones in another currency. "In stock" counts units on
  hand; leave it empty if you don't count stock.
- **Shopping:** the catalog (categories, search), product pages (price with tax, how many
  are left), a cart kept in the visitor's browser, and checkout with name, email, optional
  phone, delivery address, a note and the terms. Payment is by bank transfer or on invoice;
  card payment needs a payment provider and is not built yet.
- **What checkout does:**
  - takes the units off the stock, all or nothing (no ordering more than is left);
  - finds the customer's contact by email, or creates one;
  - creates an **Order**, a built-in CRM record type like quotes and invoices, with its
    products as rows, the contact, and the delivery details in its comment. Its stages are
    New → Paid → Shipped → Completed, or Cancelled;
  - gives it to the next person who handles orders, in turn, and notifies them;
  - emails the customer a confirmation with what they ordered, how to pay, and a link to
    their order page (progress, items, totals, payment details until it moves past New).
- **Afterwards:** each move to another stage emails the customer. Cancelling an order puts
  its units back in stock once; reopening it takes them again. The order's record shows what
  the customer entered and links to their order page, and an invoice can be made from it.
- **Spam:** a hidden decoy field, and limits per visitor, per email address and overall.

## HR: working time and time off

Under HR in the sidebar. People with "Manage HR" manage everyone; a department head manages
the people in their department and its sub-departments (never themselves).

- **The work day:** the button in the top bar starts the day, takes a break, comes back and
  ends it; starting again after ending counts the gap as a break. A day left running ends
  at midnight on its own (and says so).
- **Working time:** your month day by day (start, end, breaks, time worked) against the
  expected hours (working days × hours a day, less holidays and approved time off).
  Managers see their team's month in figures, open anyone's timesheet, and correct a day or
  add a missing one; the reason is kept with the day and in the audit log.
- **Time off:** vacation, sick leave, personal days, business trips. A request counts only
  working days (company weekdays, without holidays), can't overlap other time off, and a
  vacation can't exceed what is left of the year's allowance (counting requests still
  waiting). It goes to the person's nearest department head (or to HR managers when there
  is none), who approves or rejects it with an optional note; the person is notified.
  People withdraw requests and cancel approved time off that hasn't started (their manager
  is told); managers can cancel any.
- **Who is away:** a month chart of everyone (or one department), with holidays and
  weekends shaded and requests still waiting outlined. Notes are shown only to the person
  and their managers.
- **HR settings:** working weekdays, hours in a working day, vacation days a year, company
  holidays, and a different allowance for one person in one year.

### Check-in

Once a day, each person says where they're working today, or that they aren't. Use **Check
in** in the top bar or HR → Check-in.

- **Working:** in the office, from home, remotely, fieldwork, or somewhere they name. They can
  tell a chat (group or direct), with an optional message, and, where the company allows it,
  share where they are: the browser's location (with an OpenStreetMap link) and an address.
  Checking in can also start their work day.
- **Not working:** I'm not feeling well, sick leave, leave of absence, vacation, or their own
  reason, optionally told to a chat. Someone who checked in as working can say later that
  they aren't after all; the chat they told hears it too.
- **One check-in a day**, which they can undo the same day (a message already posted stays).
  HR managers can clear anyone's. The choices are remembered for next time.
- **The reminder:** "Check in" in the top bar gets a red dot on working days until they check
  in. It never shows on holidays, weekends or approved time off. It goes to people who have
  checked in before and to the members of any chat a check-in was posted to. It can be
  silenced for today, for a week, or for good.
- **Department view** (HR → Check-in): pick a day to see who is working where, who isn't and
  why, who is on time off, and who hasn't checked in, live. A month grid shows each person's
  days. Members of the department see it, as do its heads (and heads above) and HR managers.
  Colleagues see where someone works; the exact place is shown only to that person and their
  managers.
- **My check-ins:** your own month. Managers can see the months of the people they manage.
- **Settings** (HR managers): turn Check-in on or off for the company, and whether people may
  share where they are.

## Drive

Under Drive in the sidebar (My uploads still lists every file you have uploaded anywhere).

- **Guests** from outside the company reach only the files of the workgroups they are in
  (the group's Files tab), add files there and edit its office documents with everyone
  else; never the company's or anyone's own files, search, links or sharing.
- **Guided tours** show newcomers around (the app, the CRM lists, tasks) once; "Take the
  tour" in the user menu plays them again.

- **My drive** is yours alone until you share something. **Company** is open to every
  employee to look at and add to; renaming, moving and deleting there is for whoever added
  the thing and people with "Manage the company drive". **Shared with me** lists what
  colleagues shared.
- **Folders and files:** make folders, upload (several at once, or drop them on the page),
  rename, move within the same drive, search by name. Names are unique within a folder.
- **Versions:** uploading a file with the name of one already in the folder (or "New version")
  adds a version; every version can be downloaded, and an old one restored as the newest.
- **Sharing** (My drive): with people and departments (with sub-departments), to view, or to
  edit (add files and new versions). A shared folder shares everything in it. People are
  notified when something is shared with them.
- **Public links** to a file, optionally expiring after some days, with a download count;
  the link is shown once and can be removed. For an office document the owner may edit, a
  link can also let anyone with it **edit it in the browser** (Collabora), under the name they
  give; each save is a version made on the owner's behalf. Removing the link (or trashing
  the file) closes those editors at once.
- **Boards** can be made in a drive folder ("Board"): they show there and open as the drive
  lets each person (looking or drawing).
- **Videos** get a copy every browser plays (H.264/AAC MP4, at most 1080p) and a poster frame,
  made in the background with ffmpeg; videos that already play everywhere keep their
  original, and ones larger than `VIDEO_CONVERT_MAX_MB` (default 2048) or longer than four
  hours aren't converted. Players use the copy once it is ready.
- **Trash:** deleted things (folders with their contents) can be put back for 30 days; then
  they and every version are deleted for good, or sooner with "Delete for good".
- Editing office documents in the browser needs a document server (e.g. Collabora Online,
  self-hosted); it is not built in.

## Accounting

Double-entry books for the company, under Accounting in the sidebar. Country-neutral: taxes are
configured, not built in for one country, and there are no country-specific returns.

- **Setup:** pick the month the financial year starts; NERP makes a starter chart of accounts
  (cash and bank, receivables, payables, tax, equity, sales, the usual expenses) in the CRM's
  base currency. Every account can be changed; ones with entries keep their type and currency.
- **The journal:** every entry balances (debits = credits), has gapless numbers, and never
  changes once posted: corrections are reversing entries. Manual entries can be kept as drafts.
  A **lock date** stops anything on or before it being posted, changed or reversed (a locked
  entry is reversed into an open day). **Opening balances** carry over from previous books.
- **Sales:** CRM invoices are posted to the books (receivables, sales, tax). Each tax rate on an
  invoice must match a sales tax; per-product income accounts are possible. A posted invoice's
  amount, products, currency and client are locked in the CRM until it is voided in the books
  (only while unpaid). Fully paid invoices move to "Paid" in the CRM.
- **Purchases:** bills from suppliers, with lines to expense or asset accounts, purchase taxes
  (prices with or without tax), the supplier's PDF attached; drafts, then posted and paid.
- **Payments** in and out through bank or cash accounts, allocated to invoices or bills (part or
  whole). Money not allocated stays on the customer's or supplier's account as an advance and
  can be allocated later. Two payments of the same invoice at once can't overpay it.
- **Other currencies:** invoices, bills, bank accounts and payments in another currency keep
  their amounts in it and are booked in the base currency at their own rate; the difference when
  they are paid at another rate goes to the exchange differences account.
- **Banking:** import statements (CSV with the usual column names, any common date and number
  style, or OFX); importing overlapping statements skips lines already there. Each line is matched
  to an entry already in the books (suggestions by amount and date), recorded as income,
  expense or a transfer (with tax included), recorded as a customer's or supplier's payment, or
  ignored. "Only in the books" lists entries no statement accounts for yet.
- **Reports** (all also as CSV): profit and loss, balance sheet, trial balance, general ledger,
  receivables and payables ageing (adding up to the receivables and payables accounts, advances
  included), tax (taxable amounts and tax on sales and purchases, net to pay), and customer or
  supplier statements.
- **Year-end closing** moves the year's profit to retained earnings and locks the year;
  reopening is moving the lock date back and reversing the closing entry.
- **Permissions:** "View the books", "Record transactions" (post, pay, reconcile, void) and
  "Administer the books" (setup, chart, taxes, opening balances, lock date, closing).
- CRM records the books refer to stay in the recycle bin instead of being deleted for good,
  merging CRM companies or contacts carries their accounting along, and the CRM's base currency
  can't change once there are entries.
- Not included: payroll, fixed-asset depreciation schedules, inventory valuation postings,
  budgets, and country-specific tax returns.

- **Cash forecast** (Accounting → Cash forecast): the money expected in and out week by week
  (4 to 52 weeks) and the bank balance it leads to, starting from the bank and cash accounts.
  - **In:** open invoices in the books (what is still owed), on the due date plus how late that
    client usually pays (learned from its paid invoices; else everyone's); CRM invoices not in
    the books yet the same way; open deals — what isn't invoiced yet × the deal's probability
    (else how often its pipeline won over the last year) — on the close date plus how long
    that client takes to pay an invoice (else 30 days). Overdue money lands in the first week.
  - **Out:** open bills (what is still owed) on their due date.
  - The balance three ways: likely (deals weighted), at best (deals in full), invoices only;
    the lowest point and the first week it would go below zero. Each week opens to show what
    makes it up; deals without a close date are listed apart.

## Workgroups and projects

Under Workgroups in the sidebar. A **workgroup** is an ongoing team space; a **project** is the
same with start and end dates. Each has tabs for its **feed**, **tasks** (board, list, Gantt),
**files**, **calendar** and **members**, and a **chat**.

- **Who sees it:** *open* (everyone can see the content and join), *closed* (listed for
  everyone; people ask to join and an owner lets them in or declines; only members see the
  content), *private* (only members know it exists).
- **Roles:** owners manage the group, its members and requests, and moderate its feed;
  members post, add tasks, files and events; viewers read.
- **Feed:** posts go to the group's members (anyone can read an open group's posts) and show
  in members' news feed marked with the group's name.
- **Files:** a drive of the group's own: owners manage everything, members add files and
  manage their own, and the drive's versions and links work as elsewhere ("Open in Drive").
  Deleting the group deletes its files.
- **Calendar:** events on the group calendar are seen by its members; guests can still be
  invited as usual.
- **Chat:** a messenger chat whose members are always the group's members and whose name
  follows the group's.

### Scrum teams

A third kind of group ("New Scrum team"). Instead of Board/List/Gantt it has:

- **Planning:** the **backlog** (the team's tasks in no sprint, ranked by dragging) and the
  planned and active **sprints** above it. Tasks are dragged between them (or moved with
  "Move to"), estimated in **story points** (planning-poker scale or any number 0–1000) and
  grouped into **epics**, which show their progress in points. Each list has a quick-add line.
- **Active sprint:** the running sprint's board (the team's columns), its dates, days left,
  goal and points done. Dropping a card into a column linked to "done" asks for every point
  of the team's **definition of done** first; the confirmation is recorded on the task.
- **Reports:** the **burndown** of any started sprint (points left per day against the ideal
  line; tasks added to or taken out of a running sprint count as scope changes), **velocity**
  (committed and completed points of the last 12 sprints, and the average of the last 3), and
  the **sprint history**.
- **Sprints:** one runs at a time. Starting one sets its end (the team's sprint length, 14 days
  by default) and its commitment (points not yet done). Completing it keeps what it held for
  history and velocity, and moves unfinished tasks to the backlog or a planned sprint.
- **Who does what:** owners and the **Scrum master** (the creator at first; set under
  "Scrum") plan sprints, set the sprint length and definition of done, and delete epics;
  members estimate, rank, move tasks and add epics; viewers read.
- **Team meetings:** "Scrum" → "Plan team meetings" puts a daily stand-up (every weekday),
  sprint planning, review or retrospective on the team's calendar with every member invited.
- A task moved to another group leaves its sprint and epic.

## Directory and single sign-on

Administration → Directory & sign-on (needs "Manage settings").

- **LDAP directory** (Active Directory, OpenLDAP, FreeIPA, …), offered only when the server is
  started with `LDAP_ENABLED=true` (otherwise the page says so and nobody signs in through LDAP,
  whatever was saved). The server address
  (`ldaps://`, or `ldap://` with optional StartTLS), a service account that may search, the
  base DN, a filter for employees, and which attributes hold the ID, email, names, phone,
  position and department. "Test connection" lists a few of the people it finds.
  - Directory employees sign in on the normal sign-in page with their **directory password**
    (NERP binds to the directory as them); they cannot change or reset it in NERP.
  - Someone in the directory **without an account** gets one the first time they sign in,
    with the Employee role and their department matched by name.
  - **Sync** (every hour, or "Sync now") adds newcomers, updates names, phone and position,
    and **deactivates people who left** the directory (their sessions end). If they come back
    they are turned on again; people an administrator deactivated stay off. A sync that finds
    nobody (a wrong base DN or filter) changes nothing.
  - An existing NERP account with the same email is **linked** to its directory entry and
    from then on uses the directory password. Keep at least one administrator with a NERP
    password, so you can still sign in if the directory is down.
  - The test suites run their own throwaway OpenLDAP server (`slapd`).
- **Single sign-on** with OpenID Connect (Microsoft Entra ID, Google Workspace, Okta,
  Keycloak, Authentik, …): the issuer URL, client ID and secret. Register the redirect URI the
  page shows (`PUBLIC_URL` + `/api/auth/sso/callback`) with the provider. The sign-in page then
  has a button.
  - People are matched by email (and afterwards by the provider's ID, so a changed email
    still finds them). Optionally, an account is created for someone signing in for the first
    time, and sign-on can be limited to some email domains.
  - The provider's sign-in takes the place of the NERP password for that button; the
    authenticator code is still asked for when the person has one or the company requires it.
    Passwords still work for accounts that have one.
  - The flow uses PKCE and a nonce; the ID token's issuer, audience, nonce and expiry are
    checked. The issuer must be https (plain http only for localhost).
- **SAML 2.0 single sign-on** (Microsoft Entra ID / ADFS, Okta, Google Workspace, Keycloak,
  Shibboleth, OneLogin, …), next to OpenID Connect, with its own button on the sign-in page.
  - Give the identity provider NERP's metadata address (`PUBLIC_URL` +
    `/api/auth/saml/metadata`), or its entity ID (the same address) and assertion consumer
    service (`/api/auth/saml/acs`, HTTP-POST). NERP makes its own key and certificate the
    first time (kept sealed with the data key), signs its requests with them and can read
    assertions encrypted for it.
  - Set the identity provider up from its metadata address ("Read again" when it changes its
    certificate) or paste the metadata. It must have a signing certificate and a sign-in
    address with the HTTP-Redirect binding.
  - Responses are accepted only when signed (SHA-256 or stronger) by the identity provider's
    certificate; only the signed part is read (against signature wrapping); the issuer,
    audience, destination, recipient, time limits and the request being answered are checked;
    each assertion is accepted once; sign-ins started at the identity provider are refused.
  - The email comes from the `email`/`mail` attribute (or a chosen one), else the NameID; names
    from `givenName` and `sn`. Matching, new accounts, allowed domains and the authenticator
    code work as for OpenID Connect.
  - Single logout is not offered: signing out of NERP ends the NERP session only.
  - Building NERP needs xmlsec and libxml2 (`libxmlsec1-dev libxml2-dev libclang-dev`; Arch:
    `xmlsec libxml2 clang`); the server needs `libxmlsec1` and `libxmlsec1-openssl` at run time.

## Lists and business processes

Under Lists in the sidebar: the company's own small databases (purchase requests, assets,
contracts…) and the processes that run on them. Approvals waiting for you are under
Approvals (with a count in the sidebar).

- **Lists** are made by people with "Manage lists" (empty, or from a starting point:
  purchase requests, company assets, contracts). Fields: text, long text, number, amount
  of money, date, choice, employee, yes/no, file, CRM record; each can be required. Values
  are checked against their field. Items are numbered, searchable and exportable as CSV.
- **Access:** everyone, or chosen people and departments (with sub-departments): readers
  see items; editors also add items and change their own; managers change any item, the
  fields, the processes and the access. Whoever made the list manages it.
- **From the feed:** "Request…" starts a process by filling in its list's form; the post goes
  to the author and whoever has to approve it (later approvers join as the process reaches
  them), who approve or reject right on the post.
- **Testing:** while testers are named on a process, only their items start it; the run log
  shows everything the process did, over a chosen period.
- **Business processes** start when an item is added, when it changes, or by hand, and run
  their steps in order: an **approval** (by named people, the author, or the author's
  department head; any one or all of them, optionally within a time; with steps for
  approved and for rejected; a rejection with no steps of its own ends the process as
  rejected), **if…** on a field (equals, differs, more, less, empty, filled; then/otherwise
  steps), **notify**, **task** (with a deadline), **set a field**, **wait** and **stop**.
  Texts can use {{title}}, {{number}}, {{author}} and field codes.
- **Running:** each item shows its processes with a step-by-step history (who decided what,
  with comments). The author is told how it ended. The starter or a list manager can cancel
  a process. A process keeps the steps it started with, so changing the design affects only
  processes started afterwards.
- **On CRM records:** CRM administrators design processes for a record type under CRM
  settings → Business processes, in the same flowchart designer, with the record's fields
  (stage, amount, responsible person, custom fields…). They start when a record is made, when
  it changes, or by hand from the record (by people who may change it); "the author's head"
  means the head of the responsible person's department. "Set a field" changes the record like
  any other change, in its history; the processes' own changes start no further processes.
  Each record shows its processes with their history, and approvals link to the record.

## Knowledge base

Handbooks, how-tos and policies, under Knowledge base in the sidebar.

- **Spaces** are made by people with "Manage the knowledge base". Each is read by everyone or
  by chosen people and departments (a department includes its sub-departments), and edited
  by chosen editors, whoever made it, and knowledge base managers. Owners change who reads
  and edits a space, or delete it.
- **Pages** form a tree inside a space (sub-pages, breadcrumbs, "In this section"). They are
  written in Markdown and shown as cleaned HTML: scripts, unsafe links and foreign ids and
  classes are removed.
- **The editor** shows the Markdown source (highlighted, with find and replace, undo, and
  lists and quotes that continue on Enter) beside a live preview rendered exactly as readers
  will see it, across the full width (the page tree steps aside); switch to writing only or
  preview only, or go full screen. A toolbar and
  shortcuts cover headings, bold, italic, strikethrough, code, lists, checklists, quotes,
  links, code blocks, tables, dividers, footnotes and callouts (note, tip, important,
  warning, caution). Pictures and files go in by button, paste or drag and drop; they are
  stored with the page, so whoever may read the page may open them. Type `[[` (or use
  "Page") to link another page by its title. Ctrl+S saves; going to another page with unsaved
  changes asks first, while reloading keeps you in the editor with them (kept in the browser); a word count and reading time show at the bottom.
- **The visual editor** (switch between Visual and Markdown above the page; the choice is
  remembered): what you see is what readers get — headings, bold, italic, lists, checklists,
  quotes, code, links, tables, pictures — and everyone editing the page in it at the same time
  sees each other's text as they type, with their cursors and names. Their changes merge
  (a shared CRDT document kept by the server, passed on through the real-time connection);
  Save makes a version for everyone. The page is still kept as Markdown. Saving in the
  Markdown editor, or restoring a version, starts the live document again from the saved page.
- **Import:** a Markdown or HTML file, or a ZIP of them — exports from Notion, Obsidian and
  the like (folders become parent pages; ids are taken out of titles) or a Confluence HTML
  export (its tree from `index.html`; only each page's own content) — becomes pages under the
  space or a chosen page, with their pictures and the links between them.
- **Reading:** headings get links to copy, longer pages show their contents beside the text,
  code is highlighted by language, and pictures and files open with your sign-in.
- **History:** every save is a version; any version can be viewed and restored (restoring
  saves it as the newest version, so nothing is lost). If someone else saved the page while
  you were editing, your save is refused instead of overwriting theirs.
- **Search** covers the titles and text of every page you can read, with a snippet.

## Training

Courses with lessons and tests, under Training in the sidebar. People with "Manage training"
make courses, assign them, grade written answers and see everyone's results. Everyone else
enrols in the courses open to them and works through them.

- **Courses** start as drafts and are published when ready. A course is open to everyone, or
  to chosen people and departments (a department includes its sub-departments). It has a
  description and an optional cover picture.
- **Lessons** are grouped into chapters. Each lesson has Markdown text, files, and an optional
  video (YouTube and Vimeo links play in the page). A lesson can open a number of days after
  the person enrolled, to pace a course. People mark lessons done as they go. Lesson files
  open only for people enrolled in the course.
- **Questions:** one answer, several answers, putting items in order, or a written answer
  graded by a person. Each question has points and an explanation shown when it is answered
  wrongly. It can be marked "must be right": the test isn't passed without it, whatever the
  score. **Self-check** questions appear inside their lesson for practice and don't count.
- **Tests** draw questions from the whole course or one lesson: all of them, or a random
  number. Question and answer order can be shuffled. Each test sets:
  - how people move through it: one by one in turn; may skip but answers are final; or go
    anywhere and change answers;
  - when the right answers are shown: after each answer, at the end, or never;
  - a pass percentage, marks by score ("Excellent" from 90 %…), an attempt limit, a wait
    between attempts, a time limit (enforced by the server), and an earlier test that must be
    passed first with a minimum score.
- **Scoring:** a question earns its points only when fully right (all the right choices, every
  item in place). Unanswered questions count as wrong. Attempts with written answers wait in
  **Grading**; whoever grades sets the points and can leave a note, and the person is told
  their result. Nobody grades their own answers.
- **Completing** a course means passing every test, or, for a course without tests, finishing
  every lesson. It issues a numbered **certificate**, ready to print, showing the best scores.
  A manager can revoke a certificate issued by mistake.
- **Assigning:** a course can be given to people or departments with an optional due date.
  They are notified, reminded two days before it's due, and can't leave it themselves.
  Overdue training is flagged.
- **Gradebook** for each course: everyone enrolled, their progress, each test's best score and
  attempts (managers can allow extra attempts), due dates and certificates.

## Help desk

Requests for help (tickets), under Help desk in the sidebar. Everyone can ask for help and
sees their own requests. People with "Work on the help desk" see and answer every ticket;
"Manage the help desk" also sets it up and can delete tickets.

- **Where tickets come from:**
  - **Employees** ask in the app. Support can also take a request for a colleague, or for a
    customer on the phone.
  - **Customers** use the public page at `/help` (it can be switched off). Each gets an email
    with a private link to their request page, where they read the answers, write again,
    close the request and rate the answer. Robots are kept out by a hidden field and limits
    per address.
  - **Email:** choose one of the CRM mailboxes as the help-desk inbox. Its mail then becomes
    tickets instead of CRM email. A message that answers one of our emails, or has `[#123]` in
    the subject and comes from that ticket's requester, joins that ticket; anything else
    opens a new one. The quoted earlier conversation is cut off, attachments are kept, and
    out-of-office replies and bounces are ignored. Employees who write in are recognised by
    their address.
- **Answering:** replies go to the requester (by email through the inbox, threaded so they can
  just reply, or as a notice with their link when there is no inbox). **Notes** are seen by
  support only. Each answer can log the time spent, set a status, or close the ticket.
  **Frequently used answers** can be put into a reply.
- **Details:** category, criticality, status and difficulty come from lists the managers edit
  (each with a default). A category can name the person its tickets go to. Every change to a
  ticket is kept in its history, which only support sees. Tickets from known customers link
  to their CRM contact.
- **SLAs** set how soon support must reply, counted in working hours: per weekday, up to two
  spans a day (for a lunch break), in the help desk's time zone, skipping company holidays
  from HR. Each SLA covers chosen categories, criticalities and employees or customers; a
  ticket gets the matching one with the highest priority, else the default.
  - The clock starts when the customer writes and support has nothing pending. Support's reply
    stops it (notes don't). Putting a ticket on hold stops it; taking it off hold starts it
    again from then. A new category or criticality can mean a different SLA, and the deadline
    is counted again.
  - A warning goes to the responsible person before the deadline. When it passes, the ticket
    is flagged overdue, and the responsible person and the SLA's person are told. A reply after
    the deadline is marked late.
- **Closing:** a ticket waiting for the customer closes itself after a number of days (set in
  the settings), and the customer is told. The requester is asked to rate the answer.
  Writing to a closed ticket reopens it (for customers, only if the settings allow it).
- **Lists** for support: open, assigned to me, unassigned, overdue, closed, the requester's own
  requests and spam, ordered by deadline, with search by title, number or email.
- **Statistics** for a period: tickets created and closed, open, overdue and unassigned now,
  average first reply and resolution times, late replies, time spent, ratings, tickets by
  category, status, criticality and person, and a daily chart.

## Meetings

Planning, holding and following up meetings, under Meetings in the sidebar. Everyone can
organise one.

- **People:** the organiser runs the meeting, a keeper writes the minutes, members take part.
  A meeting can belong to a workgroup, whose members can then read it. The meeting is an
  event on everyone's calendar, kept in step when its time, place or people change, and its
  chat (from the calendar event) is where the video call happens.
- **Preparing:** the agenda has topics and sub-points, each with who presents it and how
  long it takes. Members propose topics; whoever proposed one may reword or withdraw it while
  the meeting is being prepared. Presenters are asked to write a report on their topic, and
  people running the meeting set the order.
- **Holding it:** once started, the owner or keeper marks who is present, records the decision
  on each topic, marks topics settled, and makes a task from a topic (with the decision and a
  link back; in the meeting's workgroup when the person making it works there). Notes for
  the minutes can be added at any time.
- **Closing:** closing sends everyone the minutes: attendance, each topic with its reports,
  decision and task, and the notes, on a page ready to print. A closed meeting stays as it was
  (only the notes can be corrected); its owner can reopen it.
- **Series:** "plan the next meeting" copies the people, place and workgroup, and moves every
  topic that was not settled to the new agenda, marking it "carried" in the first. A topic
  keeps its history across the series: what was reported and decided each time.
- **Notifications:** invitations, being asked to present a topic, a meeting moved or cancelled,
  and the minutes.

## News feed

Company news, under News feed in the sidebar.

- **Posts** go to everyone or to chosen people and departments (a department includes its
  sub-departments); only that audience sees them, and their files. A post can carry files
  (up to 20) and a poll (two to ten answers; one vote each, changeable; results appear once
  you have voted).
- **Important posts** notify their audience and stay at the top, under "Important, not read
  yet", until each person clicks "I've read it". The author sees who has read it and who
  hasn't.
- **Likes and comments.** The author and earlier commenters are notified of new comments.
- **Editing:** authors edit and delete their posts and comments; people who manage settings
  can delete any. New posts are announced live ("1 new post"); search finds posts by text.
- **Requests:** "Request…" starts one of the company's processes (see Lists and business
  processes); its approvers decide on the post.
- **Video announcements:** "Video" records from the camera in the browser (up to five
  minutes; watch it, record again or use it); it goes on the post and plays there for the
  post's readers.
- **A board for the post:** "Board" makes a whiteboard with the post; its readers draw on it
  or only look, as the author chooses.

## Messenger

Chat with colleagues, under Messenger in the sidebar (with the number of unread messages).

- **One-to-one chats:** one per pair of people; "Message" on an employee's profile opens it,
  or New → "With one person": type or paste a name (Enter picks the highlighted match; a
  pasted full name is picked when only one person has it), then "Start chat" or "📹 Video
  call". A chat you already have with them opens.
- **Group chats:** a name, a description and members. Whoever made it is its **owner** and
  can make others **admins** or hand it over; the owner and admins change its settings and
  remove people, any member adds people, and anyone can leave (an owner who leaves hands it to
  an admin, or else to the longest-standing member). "Who writes" can be everyone or only the
  owner and admins. Notes in the chat say who made, renamed, added, removed or left. A
  one-to-one chat can't become a group; start a group instead.
- **Open chats:** a chat or channel can be *open*: the 🔍 button lists open ones you're not in;
  you can read one before joining, and "Join" adds you.
- **Channels** (New → Channel): the owner and admins post; members read, react and
  **comment** under each post (the comments open beside the channel). A channel can be private,
  open, for a department, or for the whole company.
- **The general chat** ("🏢 General chat" under the list): everyone in the company is in it,
  new employees join it and deactivated ones leave it by themselves; administrators manage it.
- **Department chats and channels:** a department's head (or whoever manages the company
  structure) makes one for the department; its members are the department's and its
  sub-departments' people, now and later. Administrators can make chats for everyone.
- **Polls** (📊): a question with 2–10 answers, one or several allowed, optionally anonymous;
  results show live with who voted (unless anonymous); whoever asked (or a manager) closes it.
- **Reactions** (☺ beside a message): 👍 ❤️ 😂 🎉 😮 😢 👏 🙏, with who reacted; the author is told.
- **Voice messages and video notes** (🎙️ and 📹 when the box is empty) are recorded in the
  browser (up to 5 minutes and 1 minute) and play in place. **Stickers** (☺ in the box).
- **Tasks and events from a chat:** ☑ and 📅 in the chat's header make a task or an event from
  the chat, and the same buttons beside a message make one from it (its first line is the
  title and its text, quoted, the description). An event invites the chat's members.
- **Folders:** "+" beside All / Unread / Channels makes a folder of chats (double-click a
  folder to change it). **Mute** a chat (its messages no longer count in the sidebar total).
- **History:** "Clear history for me" hides what came before for you; the owner and admins
  can delete it for everyone, and can have messages deleted automatically after an hour, a
  day, a week, a month or a year.
- **Chats of workgroups, tasks and events:** a workgroup, a task and a calendar event each have
  a chat of their own, made the first time someone presses "Chat" on it. Its members are the
  people on it (the group's members; the task's creator, responsible person, assistants and
  watchers; the event's organiser and guests) and follow every change, as does its name. It
  can't be renamed or have its members changed in the Messenger, links back to what it
  belongs to ("Open task" and so on), and goes when the task or event is deleted. Only the
  people on it can open it.
- **Messages:** text and files (up to 10 per message; only the chat's members can open
  them), editable and deletable by their author ("Message deleted" stays in place). Enter
  sends, Shift+Enter starts a new line. Older messages load on request.
- **Live:** new messages, edits, deletions, members and read positions arrive over the
  real-time connection. Unread counts are per chat and in the sidebar; an open chat is read
  as messages arrive. Your last message shows "Seen" once the others have read it (in a
  group, by whom).

## Video calls

Calls between colleagues in the browser. "📹 Call" is in every Messenger chat, in New →
"With one person", on an employee's profile, and on a workgroup, a task and a calendar event
(for the people on it, when there is someone else to call): those start or join the call in
its chat.

- **Starting:** everyone in the chat is invited (up to 8 people); their browsers ring ("Join"
  or "Not now") and they get a notification. Starting a call in a chat that already has one
  going joins it. The chat notes when a call started and how long it lasted.
- **In the call:** a tile per person, microphone and camera on/off, screen sharing, leave.
  Someone without a camera or microphone can still watch and listen.
- **How it works:** browsers connect to each other directly (WebRTC, every participant to
  every other); NERP only passes their setup messages along, checks who is in the call, and
  drops a browser that stops sending heartbeats (closed tab, lost connection). The call ends
  when the last person leaves.
- **Across networks:** on one network (office LAN, VPN) calls connect directly. For people on
  different networks, run a TURN server (for example coturn, on your own machine) and set
  `WEBRTC_ICE_SERVERS`, e.g.
  `[{"urls":"turn:turn.example.com:3478","username":"nerp","credential":"…"}]`.
  A minimal coturn setup: `turnserver --lt-cred-mech --user=nerp:<password>
  --realm=example.com --listening-ip=<public IP> --min-port=49160 --max-port=49260` (open UDP
  3478 and the relay ports in the firewall; add TLS with `--cert`/`--pkey` for `turns:`). The
  test suite `ui40` checks that calls work when only relayed connections are possible.
- **Through a LiveKit media server** (optional, `LIVEKIT_URL`): when one is configured and
  answers as a call starts, the call goes through it. Each browser sends its picture and sound
  once and LiveKit forwards them, so calls can have up to 50 people and weak connections get a
  lighter stream. Without it, or when it is down, calls connect directly as above. LiveKit has
  its own TURN, so coturn isn't needed with it.
- **Recording** (calls through LiveKit, with `LIVEKIT_RECORDINGS_DIR` set and LiveKit Egress
  running): anyone in the call can press Record; everyone sees a "Recording · started by …"
  badge and the chat notes it. Anyone in the call can stop it, and the call ending stops it.
  Egress writes an MP4 of everyone in a grid with all the sound mixed. NERP then stores it and posts it in the
  call's chat (from whoever recorded it), and tells them it is ready. Recordings are kept like
  other files. A recording that fails is noted in the chat, and if Egress isn't running the
  Record button says so.
- **Playing and downloading recordings:** files are sent from disk as they are read (a 400 MB
  file doesn't raise the server's memory), with byte ranges so videos can be played and seeked.
  The browser downloads private files through short-lived download links (10 minutes, for one
  file, ended by signing out or deactivation), so large files go straight to disk; recordings
  in a chat have a Play button.
- **Building LiveKit Egress from source** (instead of its container): build the template in
  `template-default` (`pnpm install && pnpm build`, copied into `cmd/server/templates`), then
  `CGO_ENABLED=1 go build -o egress ./cmd/server` against the system GStreamer. At runtime it
  needs GStreamer's base, good, bad, ugly and libav plugins, Xvfb, Chromium and a PulseAudio
  server (PipeWire's works).
- **Setting up LiveKit:** run `livekit-server` with Redis and an API key/secret
  (`keys: { <key>: <secret> }` in its config), and LiveKit Egress pointed at the same Redis
  and key, writing to a folder NERP can read. Then set `LIVEKIT_URL` (what browsers reach,
  `wss://…` in production), `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`,
  `LIVEKIT_RECORDINGS_DIR`, and `LIVEKIT_API_URL`/`LIVEKIT_EGRESS_DIR` if NERP reaches LiveKit
  or Egress sees the folder at another address or path. Egress runs Chrome to draw the layout:
  allow about one CPU core per recording in progress.

## Calendar

Everyone's own calendar, under Calendar in the sidebar, with month and week views.

- **Events:** title, start and end (or whole days), where, notes, guests (colleagues), a
  reminder, a colour, and optionally the CRM record it is about (which gets a timeline
  entry). Only the organiser changes an event.
- **Guests** are notified when invited, when the event moves (their answers reset) and when
  it is cancelled. They answer Going, Maybe or Not going; the organiser is told.
- **Repeats:** daily, weekly (on chosen days) or monthly, optionally until a date. Repeats
  keep their local time across daylight-saving changes. One occurrence can be deleted
  without the rest.
- **Also shown** (switchable): your task deadlines and CRM calls and meetings, linking to
  them.
- **Colleagues' calendars** can be shown beside yours, each in its own colour; their private
  events appear only as "Busy".
- **Reminders** arrive as notifications (owner and guests who haven't declined), once per
  occurrence.
- **Subscribe:** a private link (`…/api/public/calendar/<token>.ics`) adds your events and
  CRM calls and meetings to Google Calendar, Outlook or a phone. The link is shown once;
  making a new one stops the old one, and it can be turned off.
- **The employee list in contacts apps:** with the same app password as the calendar, a
  CardDAV account (`…/dav/`, found from `/.well-known/carddav`) gives phones and mail
  programs an "Employees" address book — everyone working at the company, with job title,
  department, emails and phones — kept up to date (people who leave drop out). It is
  read-only. For Outlook, Employees → "vCards" downloads the list as a file to import.

### Host controls and call history

- Whoever started a call is its **host** ("Host" beside the call's buttons): "Mute everyone"
  turns the others' microphones off (they may turn them back on), and the host can stop the
  others using their microphone, camera or screen sharing (their buttons are then off).
- **Call history** (🕒 in the Messenger, or `/calls`): every call you were on or invited to,
  who called, the chat, when and for how long, with missed calls in red ("Missed only" lists
  just those) and "📹 Call" to call back.

## Booking

Appointments with the company's people, rooms and equipment, under Booking in the sidebar.
Everyone sees and makes bookings; the "Manage booking" permission sets up what can be booked,
its services and the self-booking forms (Booking → Settings).

- **Resources:** an employee (their bookings go on their calendar), a room, equipment or
  anything else. Each has working hours per weekday in its own time zone, days off, a capacity
  (how many bookings may overlap), and the rules clients book by: slot length, a buffer after
  each booking, minimum notice and how far ahead. A resource with bookings to come can't be
  deleted.
- **Services:** a duration (minutes, up to whole days) and a price, optionally limited to
  some resources, with **extras** that can be added to a booking.
- **The schedule:** one day, one row per resource, from the earliest opening to the latest
  closing. Click an empty time to book; click a booking to open it. The day is kept in the
  address (`/booking?day=2026-11-17`), so a reload or a shared link stays on it.
- **Bookings** take one or more resources at once and have a client. Status: booked →
  confirmed → came or no-show, or cancelled. The same time can't be taken twice beyond the
  resource's capacity; where a resource allows overbooking, staff may book over it and the
  booking is marked as overbooked.
- **Waiting list:** when nothing is free, the client can join it. When a booking is cancelled,
  the resource's person is told the time is free, and a waiting booking can then be placed.
- **Self-booking forms** at `/book/<slug>`: the client picks a service (and extras), a day and a
  free time (with who is free), and leaves their name, email and, if the form requires it,
  phone. Bots that fill the hidden field are refused. The client gets a confirmation email
  with a private link to see, confirm or cancel the booking, and reminders (below).
  Switching a form off takes it offline.
- **CRM:** with "make a contact" on, each client becomes a contact; with a deal pipeline
  chosen, each booking also gets a deal with the client, the price and the person booked.
  - A **Bookings** card on contacts, companies (their contacts' and deals' bookings) and deals.
    "Book" there offers the free times of a resource and service and books for the record's
    client; booked from a deal, the booking is linked to that deal (no new deal is made).
  - Booking events go on the contact's and deal's timelines and are **automation triggers**
    ("A booking of the client changes": made, confirmed by the client, came, didn't come,
    cancelled, or any), so a deal can move on when the client came.
- **Messages to clients:** the confirmation, a reminder the set hours before plus up to five
  more (e.g. 72 and 2 hours) — when several fall due at once only the nearest is sent — a
  follow-up the set hours after a visit marked "came", and a message to clients marked as not
  having come. Each is sent once.
- **Elsewhere:** an **Online booking** block for websites (it embeds a booking form and grows
  with it), and a "Booking link…" picker in live chat that puts a form's address in the reply.

## Live chat (contact centre)

A chat button for the company's website, answered from NERP under Contact centre.

- **Chat buttons:** set up by people with "Manage live chat": a name, the title and greeting
  visitors see, a colour, whether to ask the visitor's name and email first, and the
  operators who answer. Each button gives a line of code for the website
  (`<script src="…/chat.js" data-chat="…" async></script>`) that adds a round button in the
  corner opening the chat; "Try it" opens the chat page directly.
- **Visitors** chat without signing in; their browser keeps the chat, so it survives moving
  between pages. They see operators by first name, and can end the chat or start a new one.
- **The first message** links the chat to the CRM (the contact or lead with the visitor's
  email, or a new lead with the message as its comment and the page they were on), gives it
  to the next active operator in turn, and notifies them.
- **Chats** shows waiting (nobody's yet), mine, all open and closed chats, with unread
  counts (also in the sidebar). Messages arrive live. Replying takes an unassigned chat;
  another operator can take it over. Operators see their buttons' chats; people with
  "Manage live chat" see all.
- **Ending a chat** (either side) puts its transcript on the CRM record's timeline.
- **Spam:** limits per visitor address on starting chats and per chat on messages; chats
  nobody wrote in are removed after a day.

## Websites

Simple sites for the company, a product or an event, under CRM → Websites (permission
"Build websites"), served at `PUBLIC_URL/s/<address>/<page>`.

- **A site** has a name, an address, a colour, a font, a logo and a footer. It starts with a
  home page; more pages can be added, each with its own address, a description for search
  engines, and whether it is in the menu.
- **Pages are made of blocks:** a banner (heading, text, button, background picture), text in
  Markdown, features, an image with a caption, a gallery, a call to action, contact details
  (address, phone, email, opening hours) and a **web form** from CRM → Web forms, so what
  visitors send lands in the CRM as that form says. The editor shows a live preview.
- **Drafts and publishing:** changes are saved as a draft; visitors see the last published
  version until the page is published again. Pages can be unpublished (not the home page).
  The site list shows which pages have unpublished changes.
- **Safety:** blocks keep only their own fields; links may only go to web addresses, pages of
  the site, e-mail or phone; text is cleaned like the knowledge base. Pictures must be the
  editor's own uploads, and are public only while a published page (or the logo) uses them.
- **Visits:** views per page over the last 7 and 30 days.
- A site of its own domain: point the domain at the server and have the reverse proxy
  rewrite `/` to `/s/<address>/`.

## Inventory

Warehouses, stock per warehouse, and documents that change it. Everyone can see stock;
documents and warehouses need the "Manage inventory" permission.

- **Warehouses:** one is the default: new documents start there, and "In stock" on a product
  form adds to or takes from it. A warehouse that has history can't be deleted, only
  deactivated, and only once it is empty.
- **Stock:** each product whose stock is counted, per warehouse and in total, filtered by
  warehouse, "5 or fewer" or "out of stock". Clicking a product shows its movement history:
  every change, why (document, shop order, cancelled order, product form), by whom, and the
  balance after it.
- **Documents:** drafts until posted. Posting applies the whole document or nothing; no
  warehouse can go below zero. A posted document can't be edited or deleted, but it can be
  cancelled, which undoes it (refused if the goods it brought in have since left).
  - **Receipt (RC-…):** goods arrive in a warehouse, with the supplier (a CRM company), their
    invoice number and purchase prices, so the document shows what the goods cost.
  - **Write-off (WO-…):** goods leave a warehouse (damaged, lost, used).
  - **Transfer (TR-…):** goods move between two warehouses.
  - **Stock count (IC-…):** each listed product is set to the quantity found; the document
    shows the differences.
- **The shop** sells from a chosen warehouse (CRM → Online shop; the default otherwise):
  it shows what that warehouse holds, takes ordered units from it, and gets them back when an
  order is cancelled. A product's stock counted on the product form is the total over all
  warehouses.
- **Costing** (Inventory → Stock value): every movement carries its cost in the base
  currency, shown in the movement history.
  - Goods coming in make a cost layer and update the warehouse's moving average: receipts at
    their purchase price (converted from the document's currency), transfers at the cost they
    left the other warehouse with, returns and cancelled write-offs at the cost they went out
    with, counts and the product form at the current average.
  - Goods going out cost, by the company's method, **FIFO** (the oldest layers first) or the
    **average**. Layers are used up oldest-first either way, so the method can be changed any
    time and applies from then on. Stock sold below zero is costed at the average.
  - **Stock value**: quantity, unit cost and value per product (FIFO: what the remaining layers
    cost; average: quantity × average), for all warehouses or one. **Cost of goods sold** for a
    period: sold (orders, net of returns), written off and count differences.
  - Deals' product rows (and so gross profit) cost what the stock costs for products whose
    stock is counted: the next unit out (FIFO) or the average.
  - **Posting to the books** (optional, once accounting is set up): each movement becomes a
    journal entry ("Stock" in the journal) — goods sold: cost of goods sold against
    inventory; receipts: inventory against a goods-received account (match the supplier's
    bill against it); write-offs, counts and the product form: a stock-adjustment account.
    Transfers aren't posted. A movement that can't be posted (a locked period) still moves
    the stock, and is logged.

## Public API (connecting other systems)

Admin → **API & webhooks** (shown to people with "Use the API" or "Manage webhooks").

- **API keys**
  - Anyone with the "Use the API" permission can make keys. A key acts as its creator, with
    their access rights, but only in the areas chosen for it:

    | Scope | Covers |
    | --- | --- |
    | `crm` / `crm:read` | `/api/crm/…` |
    | `tasks` / `tasks:read` | `/api/tasks…`, `/api/projects…`, `/api/task-templates…` |
    | `users:read` | `/api/users…`, `/api/departments…` (reading only) |
    | `files` | `/api/files…` |

    `…:read` scopes allow GET only. Sign-in, settings, roles and the keys themselves are
    never reachable with a key.
  - Send the key as `Authorization: Bearer nerp_…`. The secret is shown once; only its
    hash is stored.
  - Each key may make 600 requests a minute; the last use (time and address) is shown.
  - A key stops working when revoked, when it expires (optional, 1 day to 2 years), when its
    owner is deactivated, or when they lose the permission.
  - People who manage employees can see and revoke everyone's keys. Creating and revoking
    keys goes into the audit log.
- **Webhooks** ("Manage webhooks")
  - Choose events and a URL. Events: `crm.item.created`, `crm.item.updated` (with the
    changed fields, old and new), `crm.item.stage_changed`, `crm.item.deleted`,
    `crm.activity.created`, `crm.activity.completed`, `task.created`, `task.updated`,
    `task.completed`, `task.deleted`.
  - Each event is POSTed as `{id, event, occurred_at, data}` with these headers:
    - `X-NERP-Event`
    - `X-NERP-Delivery`
    - `X-NERP-Signature: sha256=<HMAC-SHA256 of the body with the webhook's secret>`
  - Network errors, 5xx and 429 are retried after 1, 5, 30, 120 and 360 minutes; other
    errors end the delivery.
  - "Send test" sends a `ping`. The delivery log shows every attempt with the response, and
    any delivery can be sent again. Logs are kept for 30 days.
  - The same address rules as automation webhooks apply: http(s) only, no redirects,
    private addresses only with `CRM_WEBHOOK_ALLOW_PRIVATE=true`.
- **Reference:** the Reference tab, or `GET /api/openapi.json` (OpenAPI 3.1, including the
  webhook payloads) for code generators and API tools.

```sh
curl -H "Authorization: Bearer nerp_…" "https://nerp.example.com/api/crm/deal/items?semantics=process&limit=50"
```

### MCP server (outside AI clients)

`POST /api/mcp` speaks the Model Context Protocol (JSON-RPC over Streamable HTTP, protocol
versions 2025-03-26 to 2025-11-25, no sessions or server stream), so AI assistants and agent
tools can work with NERP. Give the client the URL `https://nerp.example.com/api/mcp` and an
API key as `Authorization: Bearer nerp_…` (Profile → API keys shows the details).

- **Tools:** CRM (`crm_record_types`, `crm_search`, `crm_get`, `crm_timeline`, `crm_create`,
  `crm_update`, `crm_comment`, `crm_add_activity`), tasks (`tasks_search`, `task_get`,
  `projects_list`, `task_create`, `task_update`, `task_set_status`, `task_comment`) and
  employees (`employees_search`, `departments_list`).
- **Rights:** a client sees only the tools its key's scopes allow; each tool is an ordinary
  API request with the same key, so the owner's access rights, the rate limit and the audit
  trail apply as for any API call. A key with no tool here is refused.
- Browsers may call it only from NERP's own pages (other `Origin`s are refused).

## API overview

All endpoints are under `/api`, and every one except setup, login and refresh needs a
`Authorization: Bearer <token>` header. Errors are returned as
`{"error": {"code": "...", "message": "..."}}`.

```
GET  /health                     POST /auth/login | /auth/refresh | /auth/logout
GET  /setup   POST /setup        GET  /auth/me    POST /auth/change-password
GET|POST /users                  GET|PATCH /users/{id}   PUT /users/{id}/roles   POST /users/{id}/password
PATCH /profile                   GET|POST /departments   PATCH|DELETE /departments/{id}
POST /departments/{id}/members   DELETE /departments/{id}/members/{user_id}
GET /permissions                 GET|POST /roles         PATCH|DELETE /roles/{id}
GET|POST /files (multipart)      GET|DELETE /files/{id}  GET /files/{id}/content
GET|PUT /settings                GET /audit

POST /auth/login/2fa             POST /auth/forgot-password    POST /auth/reset-password
GET  /auth/invite/{token}        POST /auth/accept-invite
GET  /auth/sessions              DELETE /auth/sessions/{id}    POST /auth/sessions/revoke-others
GET  /auth/2fa                   POST /auth/2fa/setup | enable | disable | recovery-codes
POST /users/{id}/invite          POST /users/{id}/2fa/reset
GET  /users/{id}/sessions        POST /users/{id}/sessions/revoke
GET  /notifications              GET /notifications/unread-count
POST /notifications/{id}/read    POST /notifications/read-all
POST /realtime/ticket            GET /realtime/ws?ticket=…   (WebSocket)
GET  /admin/system               GET /admin/jobs             POST /admin/jobs/{id}/retry
POST /admin/email/test
GET  /admin/storage              POST /admin/storage/cleanup   (files, encryption status)
GET|POST /admin/backups          POST /admin/backups/verify    POST /admin/backups/{name}/restore
POST /admin/backups/restore-to-moment                          GET /admin/backups/{name}/database.dump
```

Tasks & Projects:

```
GET|POST /projects               GET|PATCH|DELETE /projects/{id}
POST /projects/{id}/join | leave PUT|DELETE /projects/{id}/members/{user_id}
GET|POST /tasks                  GET /tasks/counters          GET /tasks/timer
GET|PATCH|DELETE /tasks/{id}     POST /tasks/{id}/status      POST /tasks/{id}/move
POST /tasks/{id}/checklist       PATCH|DELETE /tasks/{id}/checklist/{item}
GET|POST /tasks/{id}/comments    PATCH|DELETE /tasks/{id}/comments/{comment}
POST /tasks/{id}/files           DELETE /tasks/{id}/files/{file}
POST|DELETE /tasks/{id}/watch    GET /tasks/{id}/history
GET|POST /tasks/{id}/time        DELETE /tasks/{id}/time/{entry}
POST /tasks/{id}/timer/start | stop
GET|POST /projects/{id}/stages   PATCH|DELETE /projects/{id}/stages/{stage}   POST /tasks/{id}/stage
GET /tasks/gantt                 POST /tasks/{id}/dependencies    DELETE /tasks/{id}/dependencies/{other}
GET|POST /task-templates         GET|PATCH|DELETE /task-templates/{id}
POST /task-templates/{id}/use    POST /tasks/{id}/save-as-template
```

CRM (`{type}` is `lead`, `deal`, `contact` or `company`):

```
GET /crm/config                  GET /crm/counters            GET /crm/lookup?types=&q=
GET|POST /crm/{type}/items       GET|PATCH|DELETE /crm/{type}/items/{id}
GET /crm/{type}/kanban           POST /crm/{type}/items/{id}/move   POST /crm/{type}/bulk
POST /crm/lead/items/{id}/convert
GET|POST /crm/items/{id}/timeline     PATCH|DELETE /crm/timeline/{entry}
GET|POST /crm/doc-templates      GET|PATCH|DELETE /crm/doc-templates/{id}
POST /crm/doc-templates/preview  GET /crm/doc-templates/sample
POST /crm/types   PATCH|DELETE /crm/types/{code}
GET /crm/mailboxes/oauth/providers   POST /crm/mailboxes/oauth/{google|microsoft}/start   GET /crm/mailboxes/oauth/callback
GET|POST /crm/mailboxes   POST /crm/mailboxes/test   PATCH|DELETE /crm/mailboxes/{id}   POST /crm/mailboxes/{id}/sync
GET /admin/mailboxes   POST /admin/mailboxes/bulk   PATCH|DELETE /admin/mailboxes/{id}   POST /admin/mailboxes/{id}/sync
GET|POST /crm/{type}/workflows   GET /crm/items/{id}/workflows   POST /crm/items/{id}/workflows/{template}
POST /mcp   (MCP, API keys only)
GET|POST /crm/items/{id}/emails   GET /crm/items/{id}/email-recipients   GET /crm/emails/{id}   POST /crm/emails/{id}/resend
GET /crm/automation?category_id=   POST /crm/robots   PATCH|DELETE /crm/robots/{id}
POST /crm/triggers   PATCH|DELETE /crm/triggers/{id}   GET /crm/items/{id}/automation
POST /crm/automation/webhook/{token}   (public, token is the secret)
GET|POST /crm/webforms   GET|PATCH|DELETE /crm/webforms/{id}   GET /crm/webforms/{id}/submissions   GET /crm/webforms/targets
GET /public/forms/{key}   POST /public/forms/{key}/submit   (public)
GET /crm/requisites/schema?country=   GET /crm/requisites/check?country=&code=&value=
GET|POST /crm/items/{id}/requisites   PATCH|DELETE /crm/requisites/{id}   GET|PUT /crm/items/{id}/buyer
GET|POST /crm/items/{id}/documents    GET|DELETE /crm/documents/{id}   POST /crm/documents/{id}/regenerate
GET|POST /crm/activities         PATCH|DELETE /crm/activities/{id}  POST /crm/activities/{id}/complete
GET|PUT /crm/items/{id}/products GET|POST /crm/products       PATCH|DELETE /crm/products/{id}
GET|POST /crm/currencies         PATCH|DELETE /crm/currencies/{code}
GET|POST /crm/{type}/fields      PATCH|DELETE /crm/{type}/fields/{id}
POST /crm/{type}/categories      PATCH|DELETE /crm/categories/{id}
POST /crm/categories/{id}/stages PUT /crm/categories/{id}/stage-order  PATCH|DELETE /crm/stages/{id}
GET /crm/permissions             PUT /crm/permissions/{role_id}
GET /crm/{type}/duplicates       POST /crm/{type}/merge
GET /crm/{type}/duplicate-groups POST /crm/{type}/duplicate-groups/ignore|merge
GET /crm/analytics/{type}/profit PUT /crm/automation/test-mode
GET|POST /tracking/sites         PATCH|DELETE /tracking/sites/{id}
GET|POST /tracking/links         PATCH|DELETE /tracking/links/{id}  GET /tracking/links/{id}/qr.svg
GET|PUT /tracking/spend          GET /tracking/report         GET /crm/items/{id}/journey
POST /public/track/{key}         GET /go/{code} (outside /api)
GET|POST /marketing/generators   GET|PATCH|DELETE /marketing/generators/{id}
POST /marketing/generators/{id}/run  GET /marketing/generators/{id}/preview
POST /marketing/design/preview   GET /public/m/i/{file}/{sig}
POST /calendar/slot-links/{id}/for-record
POST /crm/{type}/import          GET /crm/imports/{job}       GET /crm/{type}/export
POST /crm/migrations/analyze     GET|POST /crm/migrations      GET /crm/migrations/{id}
POST /crm/migrations/{id}/undo
GET /crm/recycle-bin             POST /crm/recycle-bin/{id}/restore  DELETE /crm/recycle-bin/{id}
GET|POST /crm/{type}/filters     DELETE /crm/filters/{id}
```

Real-time event types: `notification`, `notifications.read`, `me.changed`, `session.revoked`,
`task.changed`, `task.comment`, `task.comment_deleted`, `timer.changed`, `project.stages`, `crm.changed`.
