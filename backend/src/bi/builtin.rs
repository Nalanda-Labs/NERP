//! NERP's own data as BI datasets: one SQL query each, its columns named for the query
//! builder. Everything a dashboard shows from these is company-wide (whoever may open the
//! dashboard sees the figures, whatever their access to the records themselves).

use super::query::Ty::{self, *};

pub struct Col {
    pub key: &'static str,
    pub label: &'static str,
    pub ty: Ty,
}

pub struct Builtin {
    pub key: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub sql: &'static str,
    pub cols: &'static [Col],
}

const fn c(key: &'static str, label: &'static str, ty: Ty) -> Col {
    Col { key, label, ty }
}

pub const ALL: &[Builtin] = &[
    Builtin {
        key: "deals",
        name: "Deals",
        group: "CRM",
        sql: "SELECT i.number, i.title, c.name AS pipeline, s.name AS stage,
                CASE i.stage_semantics WHEN 'success' THEN 'Won' WHEN 'failure' THEN 'Lost' ELSE 'In progress' END AS outcome,
                bi_person(i.assigned_to) AS responsible, bi_department(i.assigned_to) AS department, i.source,
                i.opportunity_base AS amount, i.probability, co.title AS company, i.created_at, i.closed_at, i.close_date
              FROM crm_items i LEFT JOIN crm_categories c ON c.id = i.category_id LEFT JOIN crm_stages s ON s.id = i.stage_id
                LEFT JOIN crm_items co ON co.id = i.company_id
              WHERE i.entity_type = 2 AND i.deleted_at IS NULL",
        cols: &[
            c("number", "Number", Number),
            c("title", "Title", Text),
            c("pipeline", "Pipeline", Text),
            c("stage", "Stage", Text),
            c("outcome", "Outcome", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("source", "Source", Text),
            c("amount", "Amount", Number),
            c("probability", "Probability, %", Number),
            c("company", "Company", Text),
            c("created_at", "Created", Time),
            c("closed_at", "Closed", Time),
            c("close_date", "Expected close date", Date),
        ],
    },
    Builtin {
        key: "leads",
        name: "Leads",
        group: "CRM",
        sql: "SELECT i.number, i.title, s.name AS stage,
                CASE i.stage_semantics WHEN 'success' THEN 'Converted' WHEN 'failure' THEN 'Junk' ELSE 'In progress' END AS outcome,
                bi_person(i.assigned_to) AS responsible, bi_department(i.assigned_to) AS department, i.source,
                i.opportunity_base AS amount, i.created_at, i.closed_at
              FROM crm_items i LEFT JOIN crm_stages s ON s.id = i.stage_id
              WHERE i.entity_type = 1 AND i.deleted_at IS NULL",
        cols: &[
            c("number", "Number", Number),
            c("title", "Title", Text),
            c("stage", "Stage", Text),
            c("outcome", "Outcome", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("source", "Source", Text),
            c("amount", "Amount", Number),
            c("created_at", "Created", Time),
            c("closed_at", "Closed", Time),
        ],
    },
    Builtin {
        key: "contacts",
        name: "Contacts",
        group: "CRM",
        sql: "SELECT i.title AS name, co.title AS company, i.job_title, i.source, bi_person(i.assigned_to) AS responsible,
                bi_department(i.assigned_to) AS department, i.address->>'city' AS city, i.address->>'country' AS country, i.created_at
              FROM crm_items i LEFT JOIN crm_items co ON co.id = i.company_id
              WHERE i.entity_type = 3 AND i.deleted_at IS NULL",
        cols: &[
            c("name", "Name", Text),
            c("company", "Company", Text),
            c("job_title", "Job title", Text),
            c("source", "Source", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("city", "City", Text),
            c("country", "Country", Text),
            c("created_at", "Created", Time),
        ],
    },
    Builtin {
        key: "companies",
        name: "Companies",
        group: "CRM",
        sql: "SELECT i.title AS name, i.company_kind AS kind, i.industry, i.employees, i.annual_revenue, bi_person(i.assigned_to) AS responsible,
                bi_department(i.assigned_to) AS department, i.address->>'city' AS city, i.address->>'country' AS country, i.created_at
              FROM crm_items i WHERE i.entity_type = 4 AND i.deleted_at IS NULL",
        cols: &[
            c("name", "Name", Text),
            c("kind", "Kind", Text),
            c("industry", "Industry", Text),
            c("employees", "Employees", Text),
            c("annual_revenue", "Annual revenue", Number),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("city", "City", Text),
            c("country", "Country", Text),
            c("created_at", "Created", Time),
        ],
    },
    Builtin {
        key: "crm_records",
        name: "All CRM records (with smart processes)",
        group: "CRM",
        sql: "SELECT t.name_plural AS type, i.number, i.title, s.name AS stage,
                CASE i.stage_semantics WHEN 'success' THEN 'Successful' WHEN 'failure' THEN 'Failed' ELSE 'In progress' END AS outcome,
                bi_person(i.assigned_to) AS responsible, bi_department(i.assigned_to) AS department, i.opportunity_base AS amount, i.created_at, i.closed_at
              FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type LEFT JOIN crm_stages s ON s.id = i.stage_id
              WHERE i.deleted_at IS NULL",
        cols: &[
            c("type", "Type", Text),
            c("number", "Number", Number),
            c("title", "Title", Text),
            c("stage", "Stage", Text),
            c("outcome", "Outcome", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("amount", "Amount", Number),
            c("created_at", "Created", Time),
            c("closed_at", "Closed", Time),
        ],
    },
    Builtin {
        key: "activities",
        name: "CRM activities",
        group: "CRM",
        sql: "SELECT CASE a.kind WHEN 'todo' THEN 'To-do' WHEN 'call' THEN 'Call' WHEN 'meeting' THEN 'Meeting' ELSE 'Email' END AS kind,
                CASE a.direction WHEN 'incoming' THEN 'Incoming' WHEN 'outgoing' THEN 'Outgoing' END AS direction, a.subject,
                bi_person(a.responsible_id) AS responsible, bi_department(a.responsible_id) AS department, t.name AS record_type,
                a.is_completed AS done, (NOT a.is_completed AND a.deadline < now()) AS overdue, a.deadline, a.created_at, a.completed_at
              FROM crm_activities a JOIN crm_items i ON i.id = a.item_id JOIN crm_entity_types t ON t.id = i.entity_type
              WHERE i.deleted_at IS NULL",
        cols: &[
            c("kind", "Kind", Text),
            c("direction", "Direction", Text),
            c("subject", "Subject", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("record_type", "Record type", Text),
            c("done", "Done", Bool),
            c("overdue", "Overdue", Bool),
            c("deadline", "Deadline", Time),
            c("created_at", "Created", Time),
            c("completed_at", "Completed", Time),
        ],
    },
    Builtin {
        key: "orders",
        name: "Shop orders",
        group: "Sales",
        sql: "SELECT i.number, o.name AS customer, s.name AS stage,
                CASE i.stage_semantics WHEN 'success' THEN 'Completed' WHEN 'failure' THEN 'Cancelled' ELSE 'In progress' END AS outcome,
                i.opportunity_base AS amount, bi_person(i.assigned_to) AS responsible, o.created_at
              FROM shop_orders o JOIN crm_items i ON i.id = o.item_id LEFT JOIN crm_stages s ON s.id = i.stage_id
              WHERE i.deleted_at IS NULL",
        cols: &[
            c("number", "Number", Number),
            c("customer", "Customer", Text),
            c("stage", "Stage", Text),
            c("outcome", "Outcome", Text),
            c("amount", "Amount", Number),
            c("responsible", "Responsible", Text),
            c("created_at", "Placed", Time),
        ],
    },
    Builtin {
        key: "invoices",
        name: "Invoices in the books",
        group: "Sales",
        sql: "SELECT i.number, i.title, p.title AS customer, v.date, v.due_date, v.currency, v.total, round(v.total * v.fx_rate, 2) AS total_base,
                CASE v.status WHEN 'open' THEN 'Open' WHEN 'paid' THEN 'Paid' ELSE 'Void' END AS status,
                (v.status = 'open' AND v.due_date < current_date) AS overdue
              FROM acc_invoices v JOIN crm_items i ON i.id = v.item_id LEFT JOIN crm_items p ON p.id = v.party_id",
        cols: &[
            c("number", "Number", Number),
            c("title", "Title", Text),
            c("customer", "Customer", Text),
            c("date", "Date", Date),
            c("due_date", "Due", Date),
            c("currency", "Currency", Text),
            c("total", "Total", Number),
            c("total_base", "Total in base currency", Number),
            c("status", "Status", Text),
            c("overdue", "Overdue", Bool),
        ],
    },
    Builtin {
        key: "tasks",
        name: "Tasks",
        group: "Tasks",
        sql: "SELECT t.number, t.title,
                CASE t.status WHEN 'new' THEN 'New' WHEN 'in_progress' THEN 'In progress' WHEN 'review' THEN 'Under review' WHEN 'done' THEN 'Done' ELSE 'Deferred' END AS status,
                CASE t.priority WHEN 'high' THEN 'High' WHEN 'low' THEN 'Low' ELSE 'Normal' END AS priority,
                p.name AS project, bi_person(t.responsible_id) AS responsible, bi_department(t.responsible_id) AS department,
                bi_person(t.created_by) AS created_by, t.created_at, t.deadline, t.completed_at,
                (t.status NOT IN ('done', 'deferred') AND t.deadline < now()) AS overdue,
                (t.completed_at IS NOT NULL AND t.deadline IS NOT NULL AND t.completed_at > t.deadline) AS completed_late,
                round(t.time_estimate_secs / 3600.0, 2) AS estimate_hours,
                round(COALESCE((SELECT SUM(e.seconds) FROM task_time_entries e WHERE e.task_id = t.id), 0) / 3600.0, 2) AS spent_hours
              FROM tasks t LEFT JOIN projects p ON p.id = t.project_id",
        cols: &[
            c("number", "Number", Number),
            c("title", "Title", Text),
            c("status", "Status", Text),
            c("priority", "Priority", Text),
            c("project", "Workgroup", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("created_by", "Created by", Text),
            c("created_at", "Created", Time),
            c("deadline", "Deadline", Time),
            c("completed_at", "Completed", Time),
            c("overdue", "Overdue", Bool),
            c("completed_late", "Completed late", Bool),
            c("estimate_hours", "Estimate, hours", Number),
            c("spent_hours", "Time spent, hours", Number),
        ],
    },
    Builtin {
        key: "task_time",
        name: "Time spent on tasks",
        group: "Tasks",
        sql: "SELECT t.title AS task, p.name AS project, bi_person(e.user_id) AS person, bi_department(e.user_id) AS department,
                e.started_at, round(e.seconds / 3600.0, 2) AS hours
              FROM task_time_entries e JOIN tasks t ON t.id = e.task_id LEFT JOIN projects p ON p.id = t.project_id",
        cols: &[
            c("task", "Task", Text),
            c("project", "Workgroup", Text),
            c("person", "Person", Text),
            c("department", "Department", Text),
            c("started_at", "Started", Time),
            c("hours", "Hours", Number),
        ],
    },
    Builtin {
        key: "working_time",
        name: "Working time",
        group: "HR",
        sql: "SELECT bi_person(w.user_id) AS person, bi_department(w.user_id) AS department, w.day, w.started_at, w.ended_at,
                round(GREATEST(EXTRACT(EPOCH FROM (COALESCE(w.ended_at, now()) - w.started_at))
                  - COALESCE((SELECT SUM(EXTRACT(EPOCH FROM (COALESCE(b.ended_at, now()) - b.started_at))) FROM hr_work_breaks b WHERE b.day_id = w.id), 0), 0) / 3600.0, 2) AS hours,
                (w.edited_by IS NOT NULL) AS corrected
              FROM hr_work_days w",
        cols: &[
            c("person", "Person", Text),
            c("department", "Department", Text),
            c("day", "Day", Date),
            c("started_at", "Started", Time),
            c("ended_at", "Ended", Time),
            c("hours", "Hours worked", Number),
            c("corrected", "Corrected by a manager", Bool),
        ],
    },
    Builtin {
        key: "absences",
        name: "Time off",
        group: "HR",
        sql: "SELECT bi_person(a.user_id) AS person, bi_department(a.user_id) AS department,
                CASE a.kind WHEN 'vacation' THEN 'Vacation' WHEN 'sick' THEN 'Sick leave' WHEN 'personal' THEN 'Personal' WHEN 'business_trip' THEN 'Business trip' ELSE 'Other' END AS kind,
                CASE a.status WHEN 'pending' THEN 'Waiting' WHEN 'approved' THEN 'Approved' WHEN 'rejected' THEN 'Rejected' ELSE 'Cancelled' END AS status,
                a.starts_on, a.ends_on, (a.ends_on - a.starts_on + 1) AS days
              FROM hr_absences a",
        cols: &[
            c("person", "Person", Text),
            c("department", "Department", Text),
            c("kind", "Kind", Text),
            c("status", "Status", Text),
            c("starts_on", "From", Date),
            c("ends_on", "To", Date),
            c("days", "Days", Number),
        ],
    },
    Builtin {
        key: "employees",
        name: "Employees",
        group: "HR",
        sql: "SELECT bi_person(u.id) AS name, bi_department(u.id) AS department, u.position, u.is_active AS active, u.created_at AS joined, u.last_login_at
              FROM users u",
        cols: &[
            c("name", "Name", Text),
            c("department", "Department", Text),
            c("position", "Position", Text),
            c("active", "Active", Bool),
            c("joined", "Joined", Time),
            c("last_login_at", "Last signed in", Time),
        ],
    },
    Builtin {
        key: "tickets",
        name: "Help desk tickets",
        group: "Service",
        sql: "SELECT t.number, t.title, CASE t.channel WHEN 'staff' THEN 'Colleague' WHEN 'portal' THEN 'Customer portal' ELSE 'Email' END AS channel,
                cat.name AS category, cr.name AS criticality, st.name AS status, mk.name AS rating,
                bi_person(t.responsible_id) AS responsible, bi_department(t.responsible_id) AS department,
                t.created_at, t.closed_at, (t.closed_at IS NOT NULL) AS closed, t.is_overdue AS overdue, t.overdue_count AS times_overdue,
                round(EXTRACT(EPOCH FROM (t.first_response_at - t.created_at)) / 60.0, 1) AS first_response_minutes,
                t.minutes_spent, t.messages
              FROM support_tickets t
                LEFT JOIN support_dictionary cat ON cat.id = t.category_id LEFT JOIN support_dictionary cr ON cr.id = t.criticality_id
                LEFT JOIN support_dictionary st ON st.id = t.status_id LEFT JOIN support_dictionary mk ON mk.id = t.mark_id
              WHERE NOT t.is_spam",
        cols: &[
            c("number", "Number", Number),
            c("title", "Title", Text),
            c("channel", "Channel", Text),
            c("category", "Category", Text),
            c("criticality", "Criticality", Text),
            c("status", "Status", Text),
            c("rating", "Customer rating", Text),
            c("responsible", "Responsible", Text),
            c("department", "Department", Text),
            c("created_at", "Created", Time),
            c("closed_at", "Closed", Time),
            c("closed", "Closed", Bool),
            c("overdue", "Overdue now", Bool),
            c("times_overdue", "Times overdue", Number),
            c("first_response_minutes", "First answer, minutes", Number),
            c("minutes_spent", "Time spent, minutes", Number),
            c("messages", "Messages", Number),
        ],
    },
    Builtin {
        key: "live_chats",
        name: "Live chats",
        group: "Service",
        sql: "SELECT ch.name AS channel, bi_person(s.assigned_to) AS operator, CASE s.status WHEN 'open' THEN 'Open' ELSE 'Closed' END AS status,
                s.started_at, s.closed_at, round(EXTRACT(EPOCH FROM (s.closed_at - s.started_at)) / 60.0, 1) AS minutes,
                (s.item_id IS NOT NULL) AS in_crm
              FROM chat_sessions s JOIN chat_channels ch ON ch.id = s.channel_id",
        cols: &[
            c("channel", "Chat button", Text),
            c("operator", "Operator", Text),
            c("status", "Status", Text),
            c("started_at", "Started", Time),
            c("closed_at", "Closed", Time),
            c("minutes", "Length, minutes", Number),
            c("in_crm", "Linked to CRM", Bool),
        ],
    },
];

pub fn find(key: &str) -> Option<&'static Builtin> {
    ALL.iter().find(|b| b.key == key)
}
