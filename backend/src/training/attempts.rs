//! Taking tests: starting an attempt (enrolment, the required earlier test, attempts left, the
//! wait between attempts), answering (by the test's navigation rules, within its time limit),
//! finishing and scoring, grading written answers, the gradebook and extra attempts.
//!
//! Scoring: a question earns its points only when answered fully right (multiple choice: exactly
//! the right answers; order: every item in place); written answers get what the grader gives.
//! A test is passed at the pass percentage, and only with every "must be right" question right.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    check_completion,
    content::{answers_of, check_chosen, is_right, load_test, marks_of, pool_size, shuffle, Test},
    manager, require_manager, visible_course,
};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/training/tests/{id}/attempts", post(start))
        .route("/training/attempts/{id}", get(get_attempt))
        .route("/training/attempts/{id}/answers", post(answer))
        .route("/training/attempts/{id}/finish", post(finish))
        .route("/training/attempts/{id}/review", post(review))
        .route("/training/reviews", get(reviews))
        .route("/training/courses/{id}/gradebook", get(gradebook))
        .route("/training/tests/{id}/extra-attempts", post(extra_attempts))
}

#[derive(FromRow, Clone)]
struct Attempt {
    id: Uuid,
    test_id: Uuid,
    user_id: Uuid,
    started_at: DateTime<Utc>,
    deadline_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    status: String,
    score: i32,
    max_score: i32,
    percent: i32,
    passed: Option<bool>,
    mark: Option<String>,
}

const ATTEMPT_COLS: &str = "id, test_id, user_id, started_at, deadline_at, finished_at, status, score, max_score, percent, passed, mark";

async fn load(state: &AppState, id: Uuid) -> AppResult<Attempt> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {ATTEMPT_COLS} FROM training_attempts WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("attempt"))
}

/// Attempts allowed: the limit plus extra ones granted (None: unlimited).
async fn allowed(state: &AppState, t: &Test, user: Uuid) -> AppResult<Option<i64>> {
    if t.attempt_limit == 0 {
        return Ok(None);
    }
    let extra: Option<i32> = sqlx::query_scalar!("SELECT extra FROM training_extra_attempts WHERE test_id = $1 AND user_id = $2", t.id, user).fetch_optional(&state.db).await?;
    Ok(Some((t.attempt_limit + extra.unwrap_or(0)) as i64))
}

/// Why the person can't start the test now (None: they can), and when they may try again.
async fn blocker(state: &AppState, t: &Test, user: Uuid) -> AppResult<(Option<String>, Option<DateTime<Utc>>)> {
    if !t.is_active {
        return Ok((Some("this test is switched off".into()), None));
    }
    if let Some(p) = t.prerequisite_test_id {
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM training_attempts WHERE test_id = $1 AND user_id = $2 AND status = 'done' AND percent >= $3) AS "v!""#, p, user, t.prerequisite_percent)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            let title: String = sqlx::query_scalar!("SELECT title FROM training_tests WHERE id = $1", p).fetch_one(&state.db).await?;
            return Ok((Some(format!("first score at least {} % in “{title}”", t.prerequisite_percent)), None));
        }
    }
    let (used, last): (i64, Option<DateTime<Utc>>) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", MAX(finished_at) AS "c1?" FROM training_attempts WHERE test_id = $1 AND user_id = $2"#, t.id, user).map(|r| (r.c0, r.c1))
        .fetch_one(&state.db)
        .await?;
    if allowed(state, t, user).await?.is_some_and(|n| used >= n) {
        return Ok((Some("no attempts left".into()), None));
    }
    if t.wait_minutes > 0 {
        if let Some(l) = last {
            let next = l + Duration::minutes(t.wait_minutes as i64);
            if next > Utc::now() {
                return Ok((Some(format!("you can try again at {}", next.format("%H:%M UTC"))), Some(next)));
            }
        }
    }
    if pool_size(state, t.course_id, t.lesson_id).await? == 0 {
        return Ok((Some("this test has no questions yet".into()), None));
    }
    Ok((None, None))
}

/// The course's tests with where the person stands in each.
pub(super) async fn tests_for(state: &AppState, auth: &AuthUser, course: Uuid) -> AppResult<Vec<Value>> {
    let tests: Vec<Test> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM training_tests WHERE course_id = $1 {} ORDER BY sort, title",
        super::content::TEST_COLS,
        if manager(auth) { "" } else { "AND is_active" }
    )))
    .bind(course)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for t in tests {
        let (used, best, passed, open): (i64, Option<i32>, bool, Option<Uuid>) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", MAX(percent) FILTER (WHERE status = 'done') AS "c1?", COALESCE(BOOL_OR(passed), FALSE) AS "c2!",
                    (SELECT id FROM training_attempts WHERE test_id = $1 AND user_id = $2 AND status = 'in_progress' LIMIT 1) AS "c3?"
             FROM training_attempts WHERE test_id = $1 AND user_id = $2"#, t.id, auth.id).map(|r| (r.c0, r.c1, r.c2, r.c3))
        .fetch_one(&state.db)
        .await?;
        let reviewing: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM training_attempts WHERE test_id = $1 AND user_id = $2 AND status = 'review') AS "v!""#, t.id, auth.id)
            .fetch_one(&state.db)
            .await?;
        let limit = allowed(state, &t, auth.id).await?;
        let (why, next) = blocker(state, &t, auth.id).await?;
        let pool = pool_size(state, t.course_id, t.lesson_id).await?;
        out.push(json!({
            "id": t.id, "title": t.title, "description": t.description, "html": crate::kb::render(&t.description), "is_active": t.is_active,
            "questions": if t.question_count > 0 { (t.question_count as i64).min(pool) } else { pool },
            "time_limit_minutes": t.time_limit_minutes, "pass_percent": t.pass_percent, "attempt_limit": t.attempt_limit,
            "navigation": t.navigation, "show_answers": t.show_answers, "wait_minutes": t.wait_minutes,
            "prerequisite_test_id": t.prerequisite_test_id, "prerequisite_percent": t.prerequisite_percent, "lesson_id": t.lesson_id,
            "attempts_used": used, "attempts_left": limit.map(|n| (n - used).max(0)), "best_percent": best, "passed": passed,
            "in_progress": open, "waiting_review": reviewing, "can_start": why.is_none() || open.is_some(), "why_not": if open.is_some() { None } else { why }, "next_try_at": next,
        }));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Starting, answering, finishing
// ---------------------------------------------------------------------------

async fn start(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let t = load_test(&state, id).await?;
    visible_course(&state, &auth, t.course_id).await?;
    if super::enrolment(&state, auth.id, t.course_id).await?.is_none() {
        return Err(AppError::bad("enrol in the course first"));
    }
    // An attempt under way is picked up again.
    let open: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM training_attempts WHERE test_id = $1 AND user_id = $2 AND status = 'in_progress'", id, auth.id).fetch_optional(&state.db).await?;
    if let Some(a) = open {
        if expire(&state, a).await? {
            // Ran out of time while away: it is over; start afresh below if allowed.
        } else {
            return view(&state, &auth, a).await.map(Json);
        }
    }
    if let (Some(why), _) = blocker(&state, &t, auth.id).await? {
        return Err(AppError::bad(why));
    }
    let mut questions: Vec<(Uuid, String, i32)> = sqlx::query!(r#"SELECT id, kind, points FROM training_questions WHERE course_id = $1 AND NOT self_check AND ($2::uuid IS NULL OR lesson_id = $2) ORDER BY sort, created_at"#, t.course_id, t.lesson_id).map(|r| (r.id, r.kind, r.points))
    .fetch_all(&state.db)
    .await?;
    if t.question_count > 0 && (t.question_count as usize) < questions.len() {
        // A random subset, kept in course order unless shuffled below.
        let mut idx: Vec<usize> = (0..questions.len()).collect();
        shuffle(&mut idx);
        let mut keep: Vec<usize> = idx.into_iter().take(t.question_count as usize).collect();
        keep.sort();
        questions = keep.into_iter().map(|i| questions[i].clone()).collect();
    }
    if t.shuffle_questions {
        shuffle(&mut questions);
    }
    let max: i32 = questions.iter().map(|q| q.2).sum();
    let deadline = (t.time_limit_minutes > 0).then(|| Utc::now() + Duration::minutes(t.time_limit_minutes as i64));
    let mut tx = state.db.begin().await?;
    let aid: Uuid = sqlx::query_scalar!("INSERT INTO training_attempts (test_id, user_id, deadline_at, max_score) VALUES ($1, $2, $3, $4) RETURNING id", id, auth.id, deadline, max)
        .fetch_one(&mut *tx)
        .await?;
    for (i, (qid, kind, _)) in questions.iter().enumerate() {
        let mut order: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM training_answers WHERE question_id = $1 ORDER BY sort, id", qid).fetch_all(&mut *tx).await?;
        if kind == "order" {
            // Never shown already in the right order.
            let right = order.clone();
            for _ in 0..10 {
                shuffle(&mut order);
                if order != right {
                    break;
                }
            }
        } else if t.shuffle_answers {
            shuffle(&mut order);
        }
        sqlx::query!("INSERT INTO training_attempt_answers (attempt_id, question_id, position, answer_order) VALUES ($1, $2, $3, $4)", aid, qid, i as i32, &order)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    view(&state, &auth, aid).await.map(Json)
}

/// Ends an attempt whose time ran out. Returns whether it did.
async fn expire(state: &AppState, id: Uuid) -> AppResult<bool> {
    let a = load(state, id).await?;
    if a.status == "in_progress" && a.deadline_at.is_some_and(|d| d <= Utc::now()) {
        close(state, &a).await?;
        return Ok(true);
    }
    Ok(false)
}

#[derive(FromRow)]
struct Row {
    question_id: Uuid,
    position: i32,
    answer_order: Vec<Uuid>,
    answered: bool,
    chosen: Vec<Uuid>,
    written: Option<String>,
    correct: Option<bool>,
    points: i32,
    review_note: Option<String>,
    kind: String,
    text: String,
    max_points: i32,
    required_correct: bool,
    explanation: Option<String>,
}

async fn rows(state: &AppState, attempt: Uuid) -> AppResult<Vec<Row>> {
    Ok(sqlx::query_as(
        "SELECT x.question_id, x.position, x.answer_order, x.answered, x.chosen, x.written, x.correct, x.points, x.review_note,
                q.kind, q.text, q.points AS max_points, q.required_correct, q.explanation
         FROM training_attempt_answers x JOIN training_questions q ON q.id = x.question_id WHERE x.attempt_id = $1 ORDER BY x.position",
    )
    .bind(attempt)
    .fetch_all(&state.db)
    .await?)
}

/// The attempt as the person (or a manager) sees it.
async fn view(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    expire(state, id).await?;
    let a = load(state, id).await?;
    if a.user_id != auth.id && !manager(auth) {
        return Err(AppError::NotFound("attempt"));
    }
    let t = load_test(state, a.test_id).await?;
    let course: (Uuid, String) = sqlx::query!(r#"SELECT id, title FROM training_courses WHERE id = $1"#, t.course_id).map(|r| (r.id, r.title)).fetch_one(&state.db).await?;
    let finished = a.status != "in_progress";
    let reveal = match t.show_answers.as_str() {
        "never" => false,
        "after_each" => true,
        _ => finished,
    };
    let mut qs = Vec::new();
    for r in rows(state, id).await? {
        let answers = answers_of(state, r.question_id).await?;
        let shown: Vec<Value> = r
            .answer_order
            .iter()
            .filter_map(|aid| answers.iter().find(|x| x.0 == *aid))
            .map(|(i, text, _)| json!({ "id": i, "text": text }))
            .collect();
        // Feedback on an answered question: after each answer, or at the end.
        let show = reveal && (finished || r.answered);
        let right: Vec<Uuid> = if r.kind == "order" { answers.iter().map(|x| x.0).collect() } else { answers.iter().filter(|x| x.2).map(|x| x.0).collect() };
        qs.push(json!({
            "question_id": r.question_id, "position": r.position, "kind": r.kind, "html": crate::kb::render(&r.text), "points": r.max_points,
            "required_correct": r.required_correct, "answers": shown, "answered": r.answered, "chosen": r.chosen, "written": r.written,
            "correct": if finished || show { r.correct } else { None }, "earned": if finished { Some(r.points) } else { None },
            "right_answers": if show && r.kind != "text" { Some(right) } else { None },
            "explanation": if show && r.correct == Some(false) { r.explanation } else { None },
            "review_note": if finished { r.review_note } else { None },
        }));
    }
    let current = if t.navigation == "strict" { qs.iter().position(|q| q["answered"] == json!(false)) } else { None };
    Ok(json!({
        "id": a.id, "status": a.status, "started_at": a.started_at, "deadline_at": a.deadline_at, "finished_at": a.finished_at,
        "seconds_left": a.deadline_at.filter(|_| !finished).map(|d| (d - Utc::now()).num_seconds().max(0)),
        "score": if finished { Some(a.score) } else { None }, "max_score": a.max_score, "percent": if a.status == "done" { Some(a.percent) } else { None },
        "passed": a.passed, "mark": a.mark, "user_id": a.user_id,
        "test": { "id": t.id, "title": t.title, "navigation": t.navigation, "show_answers": t.show_answers, "pass_percent": t.pass_percent, "time_limit_minutes": t.time_limit_minutes },
        "course": { "id": course.0, "title": course.1 }, "questions": qs, "current": current,
    }))
}

async fn get_attempt(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    view(&state, &auth, id).await.map(Json)
}

#[derive(Deserialize)]
struct AnswerInput {
    question_id: Uuid,
    #[serde(default)]
    chosen: Vec<Uuid>,
    written: Option<String>,
}

async fn answer(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AnswerInput>) -> AppResult<Json<Value>> {
    if expire(&state, id).await? {
        return Err(AppError::bad("time is up: the test has ended"));
    }
    let a = load(&state, id).await?;
    if a.user_id != auth.id {
        return Err(AppError::NotFound("attempt"));
    }
    if a.status != "in_progress" {
        return Err(AppError::bad("this attempt has ended"));
    }
    let t = load_test(&state, a.test_id).await?;
    let all = rows(&state, id).await?;
    let Some(r) = all.iter().find(|r| r.question_id == b.question_id) else { return Err(AppError::NotFound("question")) };
    match t.navigation.as_str() {
        "strict" => {
            let first = all.iter().find(|r| !r.answered).map(|r| r.question_id);
            if first != Some(b.question_id) {
                return Err(AppError::bad("answer the questions in turn"));
            }
        }
        "skip" if r.answered => return Err(AppError::bad("answers can't be changed in this test")),
        _ => {}
    }
    let (correct, points, chosen, written) = if r.kind == "text" {
        let w = b.written.as_deref().map(str::trim).unwrap_or("");
        if w.is_empty() || w.chars().count() > 5000 {
            return Err(AppError::bad("write an answer (up to 5000 characters)"));
        }
        (None, 0, vec![], Some(w.to_string()))
    } else {
        let answers = answers_of(&state, r.question_id).await?;
        check_chosen(&r.kind, &answers, &b.chosen)?;
        let right = is_right(&r.kind, &answers, &b.chosen);
        (Some(right), if right { r.max_points } else { 0 }, b.chosen.clone(), None)
    };
    sqlx::query!("UPDATE training_attempt_answers SET answered = TRUE, chosen = $3, written = $4, correct = $5, points = $6 WHERE attempt_id = $1 AND question_id = $2", id, b.question_id, &chosen, written.as_deref(), correct, points)
        .execute(&state.db)
        .await?;
    view(&state, &auth, id).await.map(Json)
}

async fn finish(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    expire(&state, id).await?;
    let a = load(&state, id).await?;
    if a.user_id != auth.id {
        return Err(AppError::NotFound("attempt"));
    }
    if a.status == "in_progress" {
        close(&state, &a).await?;
    }
    view(&state, &auth, id).await.map(Json)
}

/// Ends the attempt: unanswered questions are wrong; written answers wait for a grader.
async fn close(state: &AppState, a: &Attempt) -> AppResult<()> {
    let done = sqlx::query!("UPDATE training_attempts SET finished_at = LEAST(now(), COALESCE(deadline_at, now())) WHERE id = $1 AND status = 'in_progress' AND finished_at IS NULL", a.id)
        .execute(&state.db)
        .await?;
    if done.rows_affected() == 0 {
        return Ok(());
    }
    sqlx::query!("UPDATE training_attempt_answers SET correct = FALSE, points = 0 WHERE attempt_id = $1 AND NOT answered", a.id).execute(&state.db).await?;
    let pending: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM training_attempt_answers WHERE attempt_id = $1 AND correct IS NULL"#, a.id).fetch_one(&state.db).await?;
    if pending > 0 {
        sqlx::query!("UPDATE training_attempts SET status = 'review' WHERE id = $1", a.id).execute(&state.db).await?;
        let t = load_test(state, a.test_id).await?;
        let name: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, a.user_id).fetch_one(&state.db).await?;
        let graders: Vec<Uuid> = sqlx::query_scalar!("SELECT DISTINCT ur.user_id FROM user_roles ur JOIN role_permissions rp ON rp.role_id = ur.role_id JOIN users u ON u.id = ur.user_id
             WHERE rp.permission IN ($1, '*') AND u.is_active", perm::TRAINING)
        .fetch_all(&state.db)
        .await?;
        notify(state, &graders, NewNotification {
            kind: "training.review",
            title: format!("{name}'s answers to grade: {}", t.title),
            body: Some(format!("{pending} written {}", if pending == 1 { "answer" } else { "answers" })),
            link: Some("/training/reviews".into()),
            actor: Some(a.user_id),
        })
        .await;
    } else {
        score(state, a.id).await?;
    }
    Ok(())
}

/// Totals a fully graded attempt: score, percent, passed and mark; then the course may be complete.
async fn score(state: &AppState, id: Uuid) -> AppResult<()> {
    let a = load(state, id).await?;
    let t = load_test(state, a.test_id).await?;
    let (earned, missed_required): (i64, bool) = sqlx::query!(r#"SELECT COALESCE(SUM(x.points), 0) AS "c0!", COALESCE(BOOL_OR(q.required_correct AND NOT COALESCE(x.correct, FALSE)), FALSE) AS "c1!"
         FROM training_attempt_answers x JOIN training_questions q ON q.id = x.question_id WHERE x.attempt_id = $1"#, id).map(|r| (r.c0, r.c1))
    .fetch_one(&state.db)
    .await?;
    let percent = if a.max_score > 0 { (earned * 100 / a.max_score as i64) as i32 } else { 100 };
    let passed = percent >= t.pass_percent && !missed_required;
    let mark = marks_of(state, t.id).await?.into_iter().find(|(p, _)| percent >= *p).map(|(_, m)| m);
    sqlx::query!("UPDATE training_attempts SET status = 'done', score = $2, percent = $3, passed = $4, mark = $5 WHERE id = $1", id, earned as i32, percent, passed, mark.as_deref())
        .execute(&state.db)
        .await?;
    if passed {
        check_completion(state, a.user_id, t.course_id).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Grading written answers
// ---------------------------------------------------------------------------

async fn reviews(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    #[allow(clippy::type_complexity)]
    let items: Vec<(Uuid, Uuid, String, i32, Option<String>, Uuid, String, Uuid, String, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT a.id, q.id, q.text, q.points, x.written, t.id, t.title, c.id, c.title, trim(u.first_name || ' ' || u.last_name), a.finished_at
         FROM training_attempt_answers x JOIN training_attempts a ON a.id = x.attempt_id JOIN training_questions q ON q.id = x.question_id
         JOIN training_tests t ON t.id = a.test_id JOIN training_courses c ON c.id = t.course_id JOIN users u ON u.id = a.user_id
         WHERE a.status = 'review' AND x.correct IS NULL ORDER BY a.finished_at, x.position",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": items.into_iter().map(|(aid, qid, text, pts, written, tid, tt, cid, ct, name, at)| json!({
            "attempt_id": aid, "question_id": qid, "html": crate::kb::render(&text), "points": pts, "written": written,
            "test": { "id": tid, "title": tt }, "course": { "id": cid, "title": ct }, "person": name, "finished_at": at,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Deserialize)]
struct ReviewInput {
    question_id: Uuid,
    correct: bool,
    /// Defaults to full points when right, none when wrong.
    points: Option<i32>,
    note: Option<String>,
}

async fn review(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ReviewInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let a = load(&state, id).await?;
    if a.status != "review" {
        return Err(AppError::bad("this attempt isn't waiting for grading"));
    }
    if a.user_id == auth.id {
        return Err(AppError::bad("someone else grades your answers"));
    }
    let r: Option<(i32, Option<bool>, String)> = sqlx::query!(r#"SELECT q.points, x.correct, q.kind FROM training_attempt_answers x JOIN training_questions q ON q.id = x.question_id WHERE x.attempt_id = $1 AND x.question_id = $2"#, id, b.question_id).map(|r| (r.points, r.correct, r.kind))
    .fetch_optional(&state.db)
    .await?;
    let (max, already, kind) = r.ok_or(AppError::NotFound("question"))?;
    if kind != "text" || already.is_some() {
        return Err(AppError::bad("that answer is already graded"));
    }
    let points = b.points.unwrap_or(if b.correct { max } else { 0 });
    if !(0..=max).contains(&points) {
        return Err(AppError::bad(format!("points from 0 to {max}")));
    }
    let note = crate::routes::clean(b.note);
    if note.as_ref().is_some_and(|n| n.chars().count() > 2000) {
        return Err(AppError::bad("the note is too long"));
    }
    sqlx::query!("UPDATE training_attempt_answers SET correct = $3, points = $4, reviewed_by = $5, review_note = $6 WHERE attempt_id = $1 AND question_id = $2", id, b.question_id, b.correct, points, auth.id, note.as_deref())
        .execute(&state.db)
        .await?;
    let pending: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM training_attempt_answers WHERE attempt_id = $1 AND correct IS NULL"#, id).fetch_one(&state.db).await?;
    if pending == 0 {
        score(&state, id).await?;
        let done = load(&state, id).await?;
        let t = load_test(&state, a.test_id).await?;
        notify(&state, &[a.user_id], NewNotification {
            kind: "training.graded",
            title: format!("“{}” graded: {} %{}", t.title, done.percent, if done.passed == Some(true) { ", passed" } else { "" }),
            body: done.mark.clone(),
            link: Some(format!("/training/attempts/{id}")),
            actor: Some(auth.id),
        })
        .await;
    }
    Ok(Json(json!({ "ok": true, "pending": pending })))
}

// ---------------------------------------------------------------------------
// Gradebook
// ---------------------------------------------------------------------------

async fn gradebook(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let c = visible_course(&state, &auth, id).await?;
    let tests: Vec<(Uuid, String, i32, bool)> = sqlx::query!(r#"SELECT id, title, attempt_limit, is_active FROM training_tests WHERE course_id = $1 ORDER BY sort, title"#, id).map(|r| (r.id, r.title, r.attempt_limit, r.is_active)).fetch_all(&state.db).await?;
    #[allow(clippy::type_complexity)]
    let people: Vec<(Uuid, String, Option<Uuid>, String, Option<chrono::NaiveDate>, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query!(r#"SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!", u.avatar_file_id AS "avatar_file_id?", e.source AS "source!", e.due_on AS "due_on?", e.enrolled_at AS "enrolled_at!", e.completed_at AS "completed_at?"
         FROM training_enrolments e JOIN users u ON u.id = e.user_id WHERE e.course_id = $1 ORDER BY u.first_name, u.last_name"#, id).map(|r| (r.id, r.c1, r.avatar_file_id, r.source, r.due_on, r.enrolled_at, r.completed_at))
    .fetch_all(&state.db)
    .await?;
    let today = Utc::now().date_naive();
    let mut rows_out = Vec::new();
    for (uid, name, avatar, source, due, enrolled, completed) in people {
        let p = super::progress(&state, uid, id).await?;
        let mut results = Vec::new();
        for (tid, _, limit, _) in &tests {
            let (used, best, passed, last): (i64, Option<i32>, bool, Option<Uuid>) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", MAX(percent) FILTER (WHERE status = 'done') AS "c1?", COALESCE(BOOL_OR(passed), FALSE) AS "c2!",
                        (SELECT id FROM training_attempts WHERE test_id = $1 AND user_id = $2 ORDER BY started_at DESC LIMIT 1) AS "c3?"
                 FROM training_attempts WHERE test_id = $1 AND user_id = $2"#, tid, uid).map(|r| (r.c0, r.c1, r.c2, r.c3))
            .fetch_one(&state.db)
            .await?;
            let extra: Option<i32> = sqlx::query_scalar!("SELECT extra FROM training_extra_attempts WHERE test_id = $1 AND user_id = $2", tid, uid).fetch_optional(&state.db).await?;
            results.push(json!({ "test_id": tid, "attempts": used, "best_percent": best, "passed": passed, "last_attempt_id": last,
                                 "extra": extra.unwrap_or(0), "attempts_left": if *limit == 0 { None } else { Some((*limit as i64 + extra.unwrap_or(0) as i64 - used).max(0)) } }));
        }
        let cert: Option<(Uuid, i64)> = sqlx::query!(r#"SELECT id, number FROM training_certificates WHERE course_id = $1 AND user_id = $2"#, id, uid).map(|r| (r.id, r.number)).fetch_optional(&state.db).await?;
        rows_out.push(json!({
            "user_id": uid, "name": name, "avatar_file_id": avatar, "source": source, "due_on": due, "enrolled_at": enrolled, "completed_at": completed,
            "overdue": completed.is_none() && due.is_some_and(|d| d < today), "progress": p, "tests": results,
            "certificate": cert.map(|(i, n)| json!({ "id": i, "number": n })),
        }));
    }
    Ok(Json(json!({
        "course": { "id": c.id, "title": c.title },
        "tests": tests.into_iter().map(|(i, t, l, a)| json!({ "id": i, "title": t, "attempt_limit": l, "is_active": a })).collect::<Vec<_>>(),
        "people": rows_out,
    })))
}

#[derive(Deserialize)]
struct ExtraInput {
    user_id: Uuid,
    extra: i32,
}

/// Sets the attempts granted beyond the limit for one person.
async fn extra_attempts(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ExtraInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let t = load_test(&state, id).await?;
    if !(0..=100).contains(&b.extra) {
        return Err(AppError::bad("0 to 100 extra attempts"));
    }
    if super::enrolment(&state, b.user_id, t.course_id).await?.is_none() {
        return Err(AppError::bad("that person isn't enrolled in the course"));
    }
    sqlx::query!("INSERT INTO training_extra_attempts (test_id, user_id, extra) VALUES ($1, $2, $3) ON CONFLICT (test_id, user_id) DO UPDATE SET extra = $3", id, b.user_id, b.extra)
        .execute(&state.db)
        .await?;
    if b.extra > 0 {
        notify(&state, &[b.user_id], NewNotification {
            kind: "training.extra",
            title: format!("You can take “{}” again", t.title),
            body: None,
            link: Some(format!("/training/courses/{}", t.course_id)),
            actor: Some(auth.id),
        })
        .await;
    }
    Ok(Json(json!({ "ok": true, "extra": b.extra })))
}
