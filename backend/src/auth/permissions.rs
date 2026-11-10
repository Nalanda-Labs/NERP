use serde::Serialize;

/// Grants every permission. Only the built-in admin role should hold it.
pub const ALL: &str = "*";

pub const USERS_VIEW: &str = "users.view";
pub const USERS_MANAGE: &str = "users.manage";
pub const DEPARTMENTS_MANAGE: &str = "departments.manage";
pub const ROLES_MANAGE: &str = "roles.manage";
pub const FILES_MANAGE: &str = "files.manage";
pub const SETTINGS_MANAGE: &str = "settings.manage";
pub const AUDIT_VIEW: &str = "audit.view";
pub const SYSTEM_MANAGE: &str = "system.manage";
pub const PROJECTS_CREATE: &str = "projects.create";
pub const TASKS_MANAGE_ALL: &str = "tasks.manage_all";
pub const CRM_ADMIN: &str = "crm.admin";
pub const CRM_PRODUCTS: &str = "crm.products";
pub const API_KEYS: &str = "api.keys";
pub const WEBHOOKS_MANAGE: &str = "webhooks.manage";
pub const MARKETING: &str = "marketing.manage";
pub const SHOP: &str = "shop.manage";
pub const INVENTORY: &str = "inventory.manage";
pub const CHAT: &str = "chat.manage";
pub const KB: &str = "kb.manage";
pub const HR: &str = "hr.manage";
pub const SITES: &str = "sites.manage";
pub const LISTS: &str = "lists.manage";
pub const DRIVE: &str = "drive.manage";
pub const BOOKING: &str = "booking.manage";
pub const TRAINING: &str = "training.manage";
pub const SUPPORT_WORK: &str = "support.work";
pub const SUPPORT_MANAGE: &str = "support.manage";
pub const ACCOUNTING_VIEW: &str = "accounting.view";
pub const ACCOUNTING_POST: &str = "accounting.post";
pub const ACCOUNTING_ADMIN: &str = "accounting.admin";

#[derive(Serialize)]
pub struct PermissionInfo {
    pub code: &'static str,
    pub group: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

/// The catalog every module adds to. Roles may only hold permissions listed here.
pub const CATALOG: &[PermissionInfo] = &[
    PermissionInfo { code: USERS_VIEW, group: "Company", name: "View employees", description: "Browse the employee directory and profiles" },
    PermissionInfo { code: USERS_MANAGE, group: "Company", name: "Manage employees", description: "Invite, edit and deactivate employees; reset passwords" },
    PermissionInfo { code: DEPARTMENTS_MANAGE, group: "Company", name: "Manage structure", description: "Create, edit and delete departments and set department heads" },
    PermissionInfo { code: PROJECTS_CREATE, group: "Tasks", name: "Create projects", description: "Start new projects and invite colleagues to them" },
    PermissionInfo { code: TASKS_MANAGE_ALL, group: "Tasks", name: "Manage all tasks and projects", description: "See, edit and delete every task and project, including private ones" },
    PermissionInfo { code: CRM_ADMIN, group: "CRM", name: "Configure CRM", description: "Pipelines, stages, custom fields, currencies and CRM access rights; full access to every CRM record" },
    PermissionInfo { code: CRM_PRODUCTS, group: "CRM", name: "Manage product catalog", description: "Create and edit products and their prices" },
    PermissionInfo { code: SITES, group: "CRM", name: "Build websites", description: "Make and publish the company's websites" },
    PermissionInfo { code: MARKETING, group: "CRM", name: "Run marketing", description: "Build segments, send email campaigns to them and manage the unsubscribe list" },
    PermissionInfo { code: CHAT, group: "CRM", name: "Manage live chat", description: "Set up website chat buttons and their operators, and see every chat" },
    PermissionInfo { code: INVENTORY, group: "CRM", name: "Manage inventory", description: "Warehouses, and documents that receive, write off, move or count stock" },
    PermissionInfo { code: SHOP, group: "CRM", name: "Run the online shop", description: "Turn the shop on or off, choose what it sells and how orders are handled" },
    PermissionInfo { code: API_KEYS, group: "Integrations", name: "Use the API", description: "Create personal API keys so other systems can work with NERP on their behalf" },
    PermissionInfo { code: WEBHOOKS_MANAGE, group: "Integrations", name: "Manage webhooks", description: "Send events (new deals, completed tasks…) to other systems' URLs" },
    PermissionInfo { code: HR, group: "Company", name: "Manage HR", description: "Approve everyone's time off, see and correct everyone's working time, and set working days, holidays and vacation allowances" },
    PermissionInfo { code: BOOKING, group: "Company", name: "Manage booking", description: "Set up what can be booked (people, rooms, equipment), its hours, services and self-booking forms" },
    PermissionInfo { code: TRAINING, group: "Company", name: "Manage training", description: "Create courses, lessons and tests, assign training, grade written answers and see everyone's results" },
    PermissionInfo { code: SUPPORT_WORK, group: "Help desk", name: "Work on the help desk", description: "See every ticket, answer customers and colleagues, write notes, change status, category and who handles it" },
    PermissionInfo { code: SUPPORT_MANAGE, group: "Help desk", name: "Manage the help desk", description: "Everything on the help desk, plus its settings: inbox, public page, categories and other lists, SLAs, frequently used answers; delete tickets" },
    PermissionInfo { code: DRIVE, group: "Company", name: "Manage the company drive", description: "Rename, move and delete anything in the company drive" },
    PermissionInfo { code: ACCOUNTING_VIEW, group: "Accounting", name: "View the books", description: "See accounts, entries, invoices and bills in the books, payments, bank statements and reports" },
    PermissionInfo { code: ACCOUNTING_POST, group: "Accounting", name: "Record transactions", description: "Post invoices, enter bills, record payments and journal entries, reconcile bank statements, void them" },
    PermissionInfo { code: ACCOUNTING_ADMIN, group: "Accounting", name: "Administer the books", description: "Set up accounting, the chart of accounts and taxes, opening balances, the lock date and year-end closing" },
    PermissionInfo { code: LISTS, group: "Company", name: "Manage lists", description: "Make lists and their business processes, and manage every list" },
    PermissionInfo { code: KB, group: "Company", name: "Manage the knowledge base", description: "Create knowledge base spaces, choose who reads and edits them, and edit every space" },
    PermissionInfo { code: ROLES_MANAGE, group: "Administration", name: "Manage roles", description: "Create roles, change their permissions and assign them to employees" },
    PermissionInfo { code: FILES_MANAGE, group: "Administration", name: "Manage all files", description: "Download and delete files uploaded by anyone" },
    PermissionInfo { code: SETTINGS_MANAGE, group: "Administration", name: "Manage settings", description: "Change company-wide settings" },
    PermissionInfo { code: AUDIT_VIEW, group: "Administration", name: "View audit log", description: "See the history of security-relevant actions" },
    PermissionInfo { code: SYSTEM_MANAGE, group: "Administration", name: "Manage system", description: "Monitor background jobs and test outgoing email" },
];

pub fn is_known(code: &str) -> bool {
    CATALOG.iter().any(|p| p.code == code)
}
