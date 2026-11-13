//! Questions (single and multiple choice, putting items in order, written answers) and tests,
//! with their marks; and the self-check questions shown inside lessons.

use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{require_enrolled, require_manager, visible_course};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/training/courses/{id}/questions", get(list_questions).post(create_question))
        .route("/training/questions/{id}", put(update_question).delete(delete_question))
        .route("/training/questions/{id}/check", post(check))
        .route("/training/courses/{id}/tests", post(create_test))
        .route("/training/tests/{id}", get(get_test).put(update_test).delete(delete_test))
}

pub const KINDS: [&str; 4] = ["single", "multiple", "order", "text"];

/// A question's answers: (id, correct, in the right order by sort).
pub(super) async fn answers_of(state: &AppState, question: Uuid) -> AppResult<Vec<(Uuid, String, bool)>> {
    Ok(sqlx::query!(r#"SELECT id, text, is_correct FROM training_answers WHERE question_id = $1 ORDER BY sort, id"#, question).map(|r| (r.id, r.text, r.is_correct)).fetch_all(&state.db).await?)
}

/// Whether `chosen` answers the question right. `answers` are in their right order.
pub(super) fn is_right(kind: &str, answers: &[(Uuid, String, bool)], chosen: &[Uuid]) -> bool {
    match kind {
        "single" | "multiple" => {
            let mut want: Vec<Uuid> = answers.iter().filter(|a| a.2).map(|a| a.0).collect();
            let mut got = chosen.to_vec();
            want.sort();
            got.sort();
            got.dedup();
            !want.is_empty() && want == got
        }
        "order" => answers.len() == chosen.len() && answers.iter().zip(chosen).all(|(a, c)| a.0 == *c),
        _ => false,
    }
}

/// Checks what was chosen fits the question: known answers, one for single, all of them (once
/// each) for order.
pub(super) fn check_chosen(kind: &str, answers: &[(Uuid, String, bool)], chosen: &[Uuid]) -> AppResult<()> {
    if chosen.iter().any(|c| !answers.iter().any(|a| a.0 == *c)) {
        return Err(AppError::bad("that isn't one of the answers"));
    }
    match kind {
        "single" if chosen.len() != 1 => Err(AppError::bad("choose one answer")),
        "multiple" if chosen.is_empty() => Err(AppError::bad("choose at least one answer")),
        "order" => {
            let mut c = chosen.to_vec();
            c.sort();
            c.dedup();
            if c.len() != answers.len() || chosen.len() != answers.len() { Err(AppError::bad("put every item in order")) } else { Ok(()) }
        }
        _ => Ok(()),
    }
}

/// Shuffles in place.
pub(super) fn shuffle<T>(v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = rand::random_range(0..=i);
        v.swap(i, j);
    }
}

// ---------------------------------------------------------------------------
// Self-check questions in lessons
// ---------------------------------------------------------------------------

pub(super) async fn self_checks(state: &AppState, lesson: Uuid, _manager: bool) -> AppResult<Vec<Value>> {
    let qs: Vec<(Uuid, String, String)> = sqlx::query!(r#"SELECT id, kind, text FROM training_questions WHERE lesson_id = $1 AND self_check ORDER BY sort, created_at"#, lesson).map(|r| (r.id, r.kind, r.text))
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for (id, kind, text) in qs {
        let mut answers: Vec<Value> = answers_of(state, id).await?.into_iter().map(|(i, t, _)| json!({ "id": i, "text": t })).collect();
        if kind == "order" {
            shuffle(&mut answers);
        }
        out.push(json!({ "id": id, "kind": kind, "html": crate::kb::render(&text), "answers": answers }));
    }
    Ok(out)
}

#[derive(Deserialize)]
struct CheckInput {
    #[serde(default)]
    chosen: Vec<Uuid>,
}

/// Try a self-check question: right or not, the explanation and the right answer.
async fn check(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CheckInput>) -> AppResult<Json<Value>> {
    let q: Option<(Uuid, String, Option<String>, bool)> = sqlx::query!(r#"SELECT course_id, kind, explanation, self_check FROM training_questions WHERE id = $1"#, id).map(|r| (r.course_id, r.kind, r.explanation, r.self_check)).fetch_optional(&state.db).await?;
    let (course, kind, explanation, self_check) = q.ok_or(AppError::NotFound("question"))?;
    visible_course(&state, &auth, course).await?;
    if !self_check {
        return Err(AppError::NotFound("question"));
    }
    require_enrolled(&state, &auth, course).await?;
    let answers = answers_of(&state, id).await?;
    check_chosen(&kind, &answers, &b.chosen)?;
    let right = is_right(&kind, &answers, &b.chosen);
    let correct: Vec<Uuid> = if kind == "order" { answers.iter().map(|a| a.0).collect() } else { answers.iter().filter(|a| a.2).map(|a| a.0).collect() };
    Ok(Json(json!({ "correct": right, "explanation": if right { None } else { explanation }, "right_answers": correct })))
}

// ---------------------------------------------------------------------------
// Questions
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Question {
    id: Uuid,
    lesson_id: Option<Uuid>,
    lesson_title: Option<String>,
    kind: String,
    text: String,
    points: i32,
    required_correct: bool,
    explanation: Option<String>,
    self_check: bool,
    sort: i32,
}

async fn list_questions(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    visible_course(&state, &auth, id).await?;
    let qs: Vec<Question> = sqlx::query_as!(Question, r#"SELECT q.id, q.lesson_id AS "lesson_id?", l.title AS "lesson_title?", q.kind, q.text, q.points, q.required_correct, q.explanation AS "explanation?", q.self_check, q.sort
         FROM training_questions q LEFT JOIN training_lessons l ON l.id = q.lesson_id WHERE q.course_id = $1 ORDER BY q.sort, q.created_at"#, id)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::new();
    for q in qs {
        let answers: Vec<Value> = answers_of(&state, q.id).await?.into_iter().map(|(i, t, c)| json!({ "id": i, "text": t, "is_correct": c })).collect();
        let mut v = json!(q);
        v["answers"] = json!(answers);
        items.push(v);
    }
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct AnswerInput {
    text: String,
    #[serde(default)]
    is_correct: bool,
}

#[derive(Deserialize)]
struct QuestionInput {
    kind: String,
    text: String,
    lesson_id: Option<Uuid>,
    points: Option<i32>,
    #[serde(default)]
    required_correct: bool,
    explanation: Option<String>,
    #[serde(default)]
    self_check: bool,
    sort: Option<i32>,
    /// Choices (single, multiple) or the items in their right order (order); none for text.
    #[serde(default)]
    answers: Vec<AnswerInput>,
}

async fn check_question(state: &AppState, course: Uuid, b: &QuestionInput) -> AppResult<(String, Option<String>)> {
    if !KINDS.contains(&b.kind.as_str()) {
        return Err(AppError::bad("kind must be single, multiple, order or text"));
    }
    let text = required("question", &b.text, 10_000)?;
    if b.points.is_some_and(|p| !(0..=1000).contains(&p)) {
        return Err(AppError::bad("points from 0 to 1000"));
    }
    if let Some(l) = b.lesson_id {
        if super::lesson_course(state, l).await? != course {
            return Err(AppError::bad("that lesson is in another course"));
        }
    }
    if b.self_check && b.lesson_id.is_none() {
        return Err(AppError::bad("a self-check question belongs to a lesson"));
    }
    if b.self_check && b.kind == "text" {
        return Err(AppError::bad("a self-check question can't be a written answer"));
    }
    if b.answers.iter().any(|a| a.text.trim().is_empty() || a.text.len() > 2000) || b.answers.len() > 30 {
        return Err(AppError::bad("each answer needs words (up to 2000 characters, 30 answers)"));
    }
    let right = b.answers.iter().filter(|a| a.is_correct).count();
    match b.kind.as_str() {
        "single" if b.answers.len() < 2 || right != 1 => return Err(AppError::bad("give at least two answers, one of them right")),
        "multiple" if b.answers.len() < 2 || right == 0 => return Err(AppError::bad("give at least two answers, at least one right")),
        "order" if b.answers.len() < 2 => return Err(AppError::bad("give at least two items, in their right order")),
        "text" if !b.answers.is_empty() => return Err(AppError::bad("a written answer has no choices")),
        _ => {}
    }
    Ok((text, clean(b.explanation.clone())))
}

async fn save_answers(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, question: Uuid, kind: &str, answers: &[AnswerInput]) -> AppResult<()> {
    sqlx::query!("DELETE FROM training_answers WHERE question_id = $1", question).execute(&mut **tx).await?;
    for (i, a) in answers.iter().enumerate() {
        sqlx::query!("INSERT INTO training_answers (question_id, text, is_correct, sort) VALUES ($1, $2, $3, $4)", question, a.text.trim(), kind == "order" || a.is_correct, i as i32)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn create_question(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<QuestionInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    visible_course(&state, &auth, id).await?;
    let (text, explanation) = check_question(&state, id, &b).await?;
    let mut tx = state.db.begin().await?;
    let qid: Uuid = sqlx::query_scalar!("INSERT INTO training_questions (course_id, lesson_id, kind, text, points, required_correct, explanation, self_check, sort)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, (SELECT COALESCE(MAX(sort), 0) + 10 FROM training_questions WHERE course_id = $1))) RETURNING id", id, b.lesson_id, &b.kind, &text, b.points.unwrap_or(10), b.required_correct, explanation.as_deref(), b.self_check, b.sort)
    .fetch_one(&mut *tx)
    .await?;
    save_answers(&mut tx, qid, &b.kind, &b.answers).await?;
    tx.commit().await?;
    list_questions(State(state), auth, Path(id)).await
}

async fn question_course(state: &AppState, id: Uuid) -> AppResult<Uuid> {
    sqlx::query_scalar!("SELECT course_id FROM training_questions WHERE id = $1", id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("question"))
}

/// Replaces the question. Attempts already made keep their grades.
async fn update_question(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<QuestionInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let course = question_course(&state, id).await?;
    let (text, explanation) = check_question(&state, course, &b).await?;
    // Answers chosen in attempts refer to the old answers: keep them when only the words change.
    let old = answers_of(&state, id).await?;
    let in_use: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM training_attempt_answers WHERE question_id = $1) AS "v!""#, id).fetch_one(&state.db).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query!("UPDATE training_questions SET lesson_id = $2, kind = $3, text = $4, points = COALESCE($5, points), required_correct = $6, explanation = $7,
            self_check = $8, sort = COALESCE($9, sort) WHERE id = $1", id, b.lesson_id, &b.kind, &text, b.points, b.required_correct, explanation.as_deref(), b.self_check, b.sort)
    .execute(&mut *tx)
    .await?;
    if in_use && old.len() == b.answers.len() {
        for (i, ((aid, _, _), a)) in old.iter().zip(&b.answers).enumerate() {
            sqlx::query!("UPDATE training_answers SET text = $2, is_correct = $3, sort = $4 WHERE id = $1", aid, a.text.trim(), b.kind == "order" || a.is_correct, i as i32)
                .execute(&mut *tx)
                .await?;
        }
    } else {
        save_answers(&mut tx, id, &b.kind, &b.answers).await?;
    }
    tx.commit().await?;
    list_questions(State(state), auth, Path(course)).await
}

async fn delete_question(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let course = question_course(&state, id).await?;
    let in_progress: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM training_attempt_answers x JOIN training_attempts a ON a.id = x.attempt_id WHERE x.question_id = $1 AND a.status <> 'done') AS "v!""#, id)
    .fetch_one(&state.db)
    .await?;
    if in_progress {
        return Err(AppError::bad("someone is taking or waiting for grading on a test with this question"));
    }
    sqlx::query!("DELETE FROM training_questions WHERE id = $1", id).execute(&state.db).await?;
    list_questions(State(state), auth, Path(course)).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub(super) struct Test {
    pub id: Uuid,
    pub course_id: Uuid,
    pub title: String,
    pub description: String,
    pub is_active: bool,
    pub lesson_id: Option<Uuid>,
    pub question_count: i32,
    pub shuffle_questions: bool,
    pub shuffle_answers: bool,
    pub navigation: String,
    pub show_answers: String,
    pub attempt_limit: i32,
    pub time_limit_minutes: i32,
    pub wait_minutes: i32,
    pub pass_percent: i32,
    pub prerequisite_test_id: Option<Uuid>,
    pub prerequisite_percent: i32,
    pub sort: i32,
}

pub(super) const TEST_COLS: &str = "id, course_id, title, description, is_active, lesson_id, question_count, shuffle_questions, shuffle_answers, navigation,
    show_answers, attempt_limit, time_limit_minutes, wait_minutes, pass_percent, prerequisite_test_id, prerequisite_percent, sort";

pub(super) async fn load_test(state: &AppState, id: Uuid) -> AppResult<Test> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TEST_COLS} FROM training_tests WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("test"))
}

pub(super) async fn marks_of(state: &AppState, test: Uuid) -> AppResult<Vec<(i32, String)>> {
    Ok(sqlx::query!(r#"SELECT min_percent, mark FROM training_test_marks WHERE test_id = $1 ORDER BY min_percent DESC"#, test).map(|r| (r.min_percent, r.mark)).fetch_all(&state.db).await?)
}

/// How many questions a test would draw from.
pub(super) async fn pool_size(state: &AppState, course: Uuid, lesson: Option<Uuid>) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM training_questions WHERE course_id = $1 AND NOT self_check AND ($2::uuid IS NULL OR lesson_id = $2)"#, course, lesson)
        .fetch_one(&state.db)
        .await?)
}

async fn test_view(state: &AppState, id: Uuid) -> AppResult<Value> {
    let t = load_test(state, id).await?;
    let pool = pool_size(state, t.course_id, t.lesson_id).await?;
    let marks = marks_of(state, id).await?;
    let mut v = json!(t);
    v["pool"] = json!(pool);
    v["marks"] = json!(marks.into_iter().map(|(p, m)| json!({ "min_percent": p, "mark": m })).collect::<Vec<_>>());
    Ok(v)
}

async fn get_test(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    test_view(&state, id).await.map(Json)
}

#[derive(Deserialize)]
struct MarkInput {
    min_percent: i32,
    mark: String,
}

#[derive(Deserialize)]
struct TestInput {
    title: String,
    #[serde(default)]
    description: String,
    is_active: Option<bool>,
    lesson_id: Option<Uuid>,
    question_count: Option<i32>,
    shuffle_questions: Option<bool>,
    shuffle_answers: Option<bool>,
    navigation: Option<String>,
    show_answers: Option<String>,
    attempt_limit: Option<i32>,
    time_limit_minutes: Option<i32>,
    wait_minutes: Option<i32>,
    pass_percent: Option<i32>,
    prerequisite_test_id: Option<Uuid>,
    prerequisite_percent: Option<i32>,
    sort: Option<i32>,
    #[serde(default)]
    marks: Vec<MarkInput>,
}

async fn check_test(state: &AppState, course: Uuid, this: Option<Uuid>, b: &TestInput) -> AppResult<String> {
    let title = required("title", &b.title, 200)?;
    if b.description.len() > 20_000 {
        return Err(AppError::bad("the description is too long"));
    }
    if b.navigation.as_deref().is_some_and(|n| !["strict", "skip", "free"].contains(&n)) {
        return Err(AppError::bad("navigation must be strict, skip or free"));
    }
    if b.show_answers.as_deref().is_some_and(|n| !["never", "after_each", "at_end"].contains(&n)) {
        return Err(AppError::bad("show answers: never, after_each or at_end"));
    }
    let pct = |v: Option<i32>| v.is_some_and(|p| !(0..=100).contains(&p));
    if pct(b.pass_percent) || pct(b.prerequisite_percent) {
        return Err(AppError::bad("percentages go from 0 to 100"));
    }
    if b.question_count.is_some_and(|n| n < 0) || b.attempt_limit.is_some_and(|n| n < 0) || b.wait_minutes.is_some_and(|n| n < 0) {
        return Err(AppError::bad("counts can't be negative"));
    }
    if b.time_limit_minutes.is_some_and(|n| !(0..=1440).contains(&n)) {
        return Err(AppError::bad("the time limit is 0 to 1440 minutes"));
    }
    if let Some(l) = b.lesson_id {
        if super::lesson_course(state, l).await? != course {
            return Err(AppError::bad("that lesson is in another course"));
        }
    }
    if let Some(p) = b.prerequisite_test_id {
        if Some(p) == this {
            return Err(AppError::bad("a test can't require itself"));
        }
        let pc: Option<Uuid> = sqlx::query_scalar!("SELECT course_id FROM training_tests WHERE id = $1", p).fetch_optional(&state.db).await?;
        if pc != Some(course) {
            return Err(AppError::bad("the required test must be in this course"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for m in &b.marks {
        if !(0..=100).contains(&m.min_percent) || m.mark.trim().is_empty() || m.mark.len() > 50 || !seen.insert(m.min_percent) {
            return Err(AppError::bad("each mark needs a name and its own percentage from 0 to 100"));
        }
    }
    Ok(title)
}

async fn save_marks(state: &AppState, test: Uuid, marks: &[MarkInput]) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM training_test_marks WHERE test_id = $1", test).execute(&mut *tx).await?;
    for m in marks {
        sqlx::query!("INSERT INTO training_test_marks (test_id, min_percent, mark) VALUES ($1, $2, $3)", test, m.min_percent, m.mark.trim()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn create_test(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TestInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    visible_course(&state, &auth, id).await?;
    let title = check_test(&state, id, None, &b).await?;
    let tid: Uuid = sqlx::query_scalar!("INSERT INTO training_tests (course_id, title, description, is_active, lesson_id, question_count, shuffle_questions, shuffle_answers, navigation,
            show_answers, attempt_limit, time_limit_minutes, wait_minutes, pass_percent, prerequisite_test_id, prerequisite_percent, sort)
         VALUES ($1, $2, $3, COALESCE($4, TRUE), $5, COALESCE($6, 0), COALESCE($7, TRUE), COALESCE($8, TRUE), COALESCE($9, 'free'), COALESCE($10, 'at_end'),
            COALESCE($11, 0), COALESCE($12, 0), COALESCE($13, 0), COALESCE($14, 70), $15, COALESCE($16, 0),
            COALESCE($17, (SELECT COALESCE(MAX(sort), 0) + 10 FROM training_tests WHERE course_id = $1)))
         RETURNING id", id, &title, b.description.trim(), b.is_active, b.lesson_id, b.question_count, b.shuffle_questions, b.shuffle_answers, b.navigation.as_deref(), b.show_answers.as_deref(), b.attempt_limit, b.time_limit_minutes, b.wait_minutes, b.pass_percent, b.prerequisite_test_id, b.prerequisite_percent, b.sort)
    .fetch_one(&state.db)
    .await?;
    save_marks(&state, tid, &b.marks).await?;
    test_view(&state, tid).await.map(Json)
}

/// Replaces the test's settings (attempts already made keep their results).
async fn update_test(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TestInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let t = load_test(&state, id).await?;
    let title = check_test(&state, t.course_id, Some(id), &b).await?;
    sqlx::query!("UPDATE training_tests SET title = $2, description = $3, is_active = COALESCE($4, is_active), lesson_id = $5, question_count = COALESCE($6, question_count),
            shuffle_questions = COALESCE($7, shuffle_questions), shuffle_answers = COALESCE($8, shuffle_answers), navigation = COALESCE($9, navigation),
            show_answers = COALESCE($10, show_answers), attempt_limit = COALESCE($11, attempt_limit), time_limit_minutes = COALESCE($12, time_limit_minutes),
            wait_minutes = COALESCE($13, wait_minutes), pass_percent = COALESCE($14, pass_percent), prerequisite_test_id = $15,
            prerequisite_percent = COALESCE($16, prerequisite_percent), sort = COALESCE($17, sort)
         WHERE id = $1", id, &title, b.description.trim(), b.is_active, b.lesson_id, b.question_count, b.shuffle_questions, b.shuffle_answers, b.navigation.as_deref(), b.show_answers.as_deref(), b.attempt_limit, b.time_limit_minutes, b.wait_minutes, b.pass_percent, b.prerequisite_test_id, b.prerequisite_percent, b.sort)
    .execute(&state.db)
    .await?;
    save_marks(&state, id, &b.marks).await?;
    test_view(&state, id).await.map(Json)
}

async fn delete_test(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let t = load_test(&state, id).await?;
    let attempts: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM training_attempts WHERE test_id = $1"#, id).fetch_one(&state.db).await?;
    if attempts > 0 {
        return Err(AppError::bad("people have taken this test: switch it off instead, so their results stay"));
    }
    sqlx::query!("DELETE FROM training_tests WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true, "course_id": t.course_id })))
}
