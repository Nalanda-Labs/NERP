// Training: courses (drafts, audience), chapters and lessons (opening later), questions of each
// kind and their checks, self-checks, enrolling, tests (navigation rules, attempts and extra
// attempts, the required earlier test, showing answers, marks, required questions, time
// limits), grading written answers, completion and certificates, the gradebook, assigning.
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';

let pass = 0, fail = 0;
const ok = (name: string, cond: unknown, extra: unknown = '') => { cond ? pass++ : fail++; console.log(cond ? 'ok  ' : 'FAIL', name, cond ? '' : String(typeof extra === 'string' ? extra : JSON.stringify(extra)).slice(0, 700)); };
const stamp = Date.now().toString(36);
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
const user = async (first: string) => {
  const u = (await admin.post('/users', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123', first_name: first, last_name: 'Learner' })).data;
  const c = new Client();
  await c.post('/auth/login', { email: `${first.toLowerCase()}.${stamp}@acme.test`, password: 'password123' });
  c.id = u.id;
  return c;
};
const ann = await user('Ann');
const bob = await user('Bob');
const tomorrow = new Date(Date.now() + 86400000).toISOString().slice(0, 10);

try {
  const dept = (await admin.post('/departments', { name: `Sales ${stamp}` })).data;
  await admin.post(`/departments/${dept.id}/members`, { user_id: ann.id });

  // ---- courses ----
  ok('only training managers make courses', (await ann.post('/training/courses', { title: 'x' })).status === 403);
  let r = await admin.post('/training/courses', { title: `Selling ${stamp}`, description: '# Welcome\nHow we sell.', to_all: false, audience: { departments: [dept.id] } });
  const course = r.data.course;
  ok('a draft course', r.status === 200 && course.is_published === false && r.data.html.includes('<h1>Welcome</h1>'), r.data);
  ok('drafts are hidden', (await ann.get(`/training/courses/${course.id}`)).status === 404 && !(await ann.get('/training/courses')).data.items.some((c: any) => c.id === course.id));
  r = await admin.post(`/training/courses/${course.id}/chapters`, { title: 'Basics' });
  const chapter = r.data.chapters[0];
  const fd = new FormData(); fd.append('file', new Blob(['Script: hello, how are you?']), `script-${stamp}.txt`);
  const file = (await admin.post('/files', fd)).data;
  const l1 = (await admin.post(`/training/courses/${course.id}/lessons`, { title: 'Greeting', chapter_id: chapter.id, body: 'Say **hello**.', video_url: 'https://video.example/1', file_ids: [file.id] })).data;
  ok('a lesson', l1.html.includes('<strong>hello</strong>') && l1.video_url === 'https://video.example/1' && l1.files[0]?.name === `script-${stamp}.txt`, l1);
  ok('video must be a web address', (await admin.post(`/training/courses/${course.id}/lessons`, { title: 'x', video_url: 'javascript:alert(1)' })).status === 400);
  const l2 = (await admin.post(`/training/courses/${course.id}/lessons`, { title: 'Closing', chapter_id: chapter.id, body: 'Ask for the order.', opens_after_days: 3 })).data;

  // ---- questions ----
  const q = (body: any) => admin.post(`/training/courses/${course.id}/questions`, body);
  ok('single needs exactly one right answer', (await q({ kind: 'single', text: 'x', answers: [{ text: 'a', is_correct: true }, { text: 'b', is_correct: true }] })).status === 400);
  ok('a written answer has no choices', (await q({ kind: 'text', text: 'x', answers: [{ text: 'a' }] })).status === 400);
  ok('a self-check belongs to a lesson', (await q({ kind: 'single', text: 'x', self_check: true, answers: [{ text: 'a', is_correct: true }, { text: 'b' }] })).status === 400);
  ok('a self-check can\'t be written', (await q({ kind: 'text', text: 'x', self_check: true, lesson_id: l1.id })).status === 400);
  await q({ kind: 'single', text: 'First words?', lesson_id: l1.id, points: 10, answers: [{ text: 'Hello', is_correct: true }, { text: 'Buy now' }, { text: 'Goodbye' }], explanation: 'Start with a greeting.' });
  await q({ kind: 'multiple', text: 'Good questions?', lesson_id: l1.id, points: 10, required_correct: true, answers: [{ text: 'What do you need?', is_correct: true }, { text: 'When can we talk?', is_correct: true }, { text: 'Why not?' }] });
  await q({ kind: 'order', text: 'Order the steps', points: 10, answers: [{ text: 'Greet' }, { text: 'Listen' }, { text: 'Offer' }, { text: 'Close' }] });
  await q({ kind: 'text', text: 'Describe your pitch', points: 10 });
  r = await q({ kind: 'single', text: 'Quick check: say?', lesson_id: l1.id, self_check: true, answers: [{ text: 'Hello', is_correct: true }, { text: 'Nothing' }], explanation: 'Always greet.' });
  ok('five questions', r.data.items.length === 5, r.data.items.length);
  const sc = r.data.items.find((x: any) => x.self_check);

  // ---- tests ----
  const t1 = (await admin.post(`/training/courses/${course.id}/tests`, {
    title: 'Final', navigation: 'skip', show_answers: 'at_end', attempt_limit: 2, pass_percent: 70, marks: [{ min_percent: 90, mark: 'Excellent' }, { min_percent: 70, mark: 'Good' }],
  })).data;
  ok('a test drawing from four questions (not the self-check)', t1.pool === 4 && t1.marks.length === 2, t1);
  ok('a test can\'t require itself', (await admin.put(`/training/tests/${t1.id}`, { title: 'Final', prerequisite_test_id: t1.id })).status === 400);
  ok('marks need their own percentages', (await admin.post(`/training/courses/${course.id}/tests`, { title: 'x', marks: [{ min_percent: 50, mark: 'A' }, { min_percent: 50, mark: 'B' }] })).status === 400);
  const t2 = (await admin.post(`/training/courses/${course.id}/tests`, {
    title: 'Greeting quiz', lesson_id: l1.id, navigation: 'strict', show_answers: 'after_each', pass_percent: 50, shuffle_questions: false, prerequisite_test_id: t1.id, prerequisite_percent: 70,
  })).data;
  ok('a lesson test draws from its lesson', t2.pool === 2, t2);

  // ---- publishing, audience, enrolling ----
  await admin.patch(`/training/courses/${course.id}`, { is_published: true });
  ok('outside the audience, it is hidden', (await bob.get(`/training/courses/${course.id}`)).status === 404);
  ok('and can\'t be joined', (await bob.post(`/training/courses/${course.id}/enrol`)).status === 404);
  r = await ann.get(`/training/courses/${course.id}`);
  ok('Ann (in Sales) sees it and may enrol', r.status === 200 && r.data.can_enrol === true && r.data.enrolment === null && r.data.lessons.every((l: any) => !l.open), r.data);
  ok('no lessons before enrolling', (await ann.get(`/training/lessons/${l1.id}`)).status === 400);
  ok('no tests before enrolling', (await ann.post(`/training/tests/${t1.id}/attempts`)).status === 400);
  r = await ann.post(`/training/courses/${course.id}/enrol`);
  ok('enrolled', r.data.enrolment.source === 'self' && r.data.progress.percent === 0, r.data);
  const lessons = r.data.lessons;
  ok('the lesson\'s file opens for Ann, enrolled', (await ann.get(`/files/${file.id}/content`)).status === 200);
  ok('not for Bob', (await bob.get(`/files/${file.id}/content`)).status !== 200);
  ok('the second lesson opens in three days', lessons.find((l: any) => l.id === l2.id).open === false && lessons.find((l: any) => l.id === l1.id).open === true);
  ok('so it can\'t be read yet', (await ann.get(`/training/lessons/${l2.id}`)).data.error?.message?.startsWith('this lesson opens on'));

  // ---- a lesson and its self-check ----
  r = await ann.get(`/training/lessons/${l1.id}`);
  const check = r.data.self_checks[0];
  ok('the lesson with its self-check, without the right answer', r.data.self_checks.length === 1 && check.answers.every((a: any) => a.is_correct === undefined) && r.data.next === l2.id, r.data);
  const hello = check.answers.find((a: any) => a.text === 'Hello').id, nothing = check.answers.find((a: any) => a.text === 'Nothing').id;
  r = await ann.post(`/training/questions/${sc.id}/check`, { chosen: [nothing] });
  ok('a wrong self-check explains', r.data.correct === false && r.data.explanation === 'Always greet.' && r.data.right_answers[0] === hello, r.data);
  ok('a right one says so', (await ann.post(`/training/questions/${sc.id}/check`, { chosen: [hello] })).data.correct === true);
  r = await ann.post(`/training/lessons/${l1.id}/complete`);
  ok('lesson done', r.data.progress.lessons_done === 1 && r.data.next === l2.id, r.data);

  // ---- the required earlier test ----
  r = await ann.get(`/training/courses/${course.id}`);
  const st2 = r.data.tests.find((t: any) => t.id === t2.id);
  ok('the quiz needs the final first', st2.can_start === false && st2.why_not.includes('Final'), st2);
  ok('starting it is refused', (await ann.post(`/training/tests/${t2.id}/attempts`)).status === 400);

  // ---- attempt 1 ----
  r = await ann.post(`/training/tests/${t1.id}/attempts`);
  let at = r.data;
  ok('an attempt with the four questions', at.status === 'in_progress' && at.questions.length === 4 && at.max_score === 40 && at.seconds_left === null, at);
  const pick = (kind: string) => at.questions.find((x: any) => x.kind === kind);
  const ids = (qq: any, texts: string[]) => texts.map((t) => qq.answers.find((a: any) => a.text === t).id);
  ok('order items are never shown in the right order', pick('order').answers.map((a: any) => a.text).join() !== 'Greet,Listen,Offer,Close');
  ok('starting again picks it up', (await ann.post(`/training/tests/${t1.id}/attempts`)).data.id === at.id);
  ok('one answer for single choice', (await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('single').question_id, chosen: ids(pick('single'), ['Hello', 'Goodbye']) })).status === 400);
  ok('order needs every item', (await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('order').question_id, chosen: ids(pick('order'), ['Greet', 'Listen']) })).status === 400);
  ok('foreign answers refused', (await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('single').question_id, chosen: [hello] })).status === 400);
  r = await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('single').question_id, chosen: ids(pick('single'), ['Hello']) });
  ok('answered; right or wrong not shown until the end', r.data.questions.find((x: any) => x.kind === 'single').answered === true && r.data.questions.find((x: any) => x.kind === 'single').correct === null, r.data.questions);
  ok('answers are final in this test', (await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('single').question_id, chosen: ids(pick('single'), ['Goodbye']) })).status === 400);
  await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('multiple').question_id, chosen: ids(pick('multiple'), ['When can we talk?', 'What do you need?']) });
  await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('order').question_id, chosen: ids(pick('order'), ['Greet', 'Listen', 'Offer', 'Close']) });
  await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('text').question_id, written: 'I listen first, then offer.' });
  ok('Bob can\'t see Ann\'s attempt', (await bob.get(`/training/attempts/${at.id}`)).status === 404);
  r = await ann.post(`/training/attempts/${at.id}/finish`);
  ok('finished: the written answer waits for grading', r.data.status === 'review' && r.data.percent === null, r.data);
  ok('Ann can\'t answer any more', (await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('text').question_id, written: 'x' })).status === 400);

  // ---- grading ----
  ok('learners don\'t grade', (await ann.get('/training/reviews')).status === 403);
  r = await admin.get('/training/reviews');
  const item = r.data.items.find((x: any) => x.attempt_id === at.id);
  ok('it is in the grading queue', item && item.written === 'I listen first, then offer.' && item.person === 'Ann Learner', r.data);
  ok('points within the question\'s', (await admin.post(`/training/attempts/${at.id}/review`, { question_id: item.question_id, correct: true, points: 11 })).status === 400);
  r = await admin.post(`/training/attempts/${at.id}/review`, { question_id: item.question_id, correct: true, points: 5, note: 'Mention the follow-up.' });
  ok('graded', r.data.pending === 0, r.data);
  at = (await ann.get(`/training/attempts/${at.id}`)).data;
  ok('35 of 40: 87 %, passed, Good', at.status === 'done' && at.score === 35 && at.percent === 87 && at.passed === true && at.mark === 'Good', at);
  ok('with the right answers and the note at the end', at.questions.find((x: any) => x.kind === 'single').right_answers.length === 1 && at.questions.find((x: any) => x.kind === 'text').review_note === 'Mention the follow-up.');
  ok('Ann was told', (await ann.get('/notifications')).data.items.some((n: any) => n.title.startsWith('“Final” graded: 87 %, passed')));

  // ---- attempt 2: a required question wrong ----
  at = (await ann.post(`/training/tests/${t1.id}/attempts`)).data;
  await ann.post(`/training/attempts/${at.id}/answers`, { question_id: pick('multiple').question_id, chosen: ids(at.questions.find((x: any) => x.kind === 'multiple'), ['Why not?']) });
  at = (await ann.post(`/training/attempts/${at.id}/finish`)).data;
  ok('unanswered and wrong: done straight away, not passed', at.status === 'done' && at.passed === false && at.percent === 0, at);
  ok('the explanation for a wrong answer', at.questions.find((x: any) => x.kind === 'single').explanation === 'Start with a greeting.');
  ok('no attempts left', (await ann.post(`/training/tests/${t1.id}/attempts`)).data.error?.message === 'no attempts left');
  ok('an extra attempt for someone enrolled only', (await admin.post(`/training/tests/${t1.id}/extra-attempts`, { user_id: bob.id, extra: 1 })).status === 400);
  await admin.post(`/training/tests/${t1.id}/extra-attempts`, { user_id: ann.id, extra: 1 });
  r = await ann.post(`/training/tests/${t1.id}/attempts`);
  ok('an extra attempt granted', r.status === 200 && r.data.status === 'in_progress', r.data);
  await ann.post(`/training/attempts/${r.data.id}/finish`);

  // ---- the quiz: strict order, answers shown after each ----
  at = (await ann.post(`/training/tests/${t2.id}/attempts`)).data;
  ok('the quiz: two questions from the lesson, in course order', at.questions.length === 2 && at.questions[0].kind === 'single' && at.current === 0, at);
  ok('answer in turn', (await ann.post(`/training/attempts/${at.id}/answers`, { question_id: at.questions[1].question_id, chosen: ids(at.questions[1], ['What do you need?']) })).data.error?.message === 'answer the questions in turn');
  r = await ann.post(`/training/attempts/${at.id}/answers`, { question_id: at.questions[0].question_id, chosen: ids(at.questions[0], ['Goodbye']) });
  ok('after each answer: right or wrong, the right answer and why', r.data.questions[0].correct === false && r.data.questions[0].right_answers.length === 1 && r.data.questions[0].explanation === 'Start with a greeting.' && r.data.current === 1, r.data.questions[0]);
  await ann.post(`/training/attempts/${at.id}/answers`, { question_id: at.questions[1].question_id, chosen: ids(at.questions[1], ['What do you need?', 'When can we talk?']) });
  at = (await ann.post(`/training/attempts/${at.id}/finish`)).data;
  ok('50 %: passed', at.percent === 50 && at.passed === true, at);

  // ---- completed: the certificate ----
  r = await ann.get(`/training/courses/${course.id}`);
  ok('the course is completed with a certificate', r.data.enrolment.completed_at && r.data.certificate?.number > 0 && r.data.progress.tests_passed === 2, r.data);
  const cert = r.data.certificate;
  r = await ann.get(`/training/certificates/${cert.id}`);
  ok('the certificate: best scores', r.data.certificate.course_title === `Selling ${stamp}` && r.data.certificate.user_name === 'Ann Learner' && r.data.certificate.score === 45 && r.data.certificate.max_score === 60, r.data);
  ok('only hers', (await bob.get(`/training/certificates/${cert.id}`)).status === 404);
  ok('in her list', (await ann.get('/training/certificates')).data.items.some((c: any) => c.id === cert.id));
  ok('she was told', (await ann.get('/notifications')).data.items.some((n: any) => n.title === `You completed “Selling ${stamp}”`));

  // ---- gradebook ----
  ok('learners don\'t see the gradebook', (await ann.get(`/training/courses/${course.id}/gradebook`)).status === 403);
  r = await admin.get(`/training/courses/${course.id}/gradebook`);
  const row = r.data.people.find((p: any) => p.user_id === ann.id);
  ok('the gradebook', row && row.completed_at && row.certificate.id === cert.id && row.tests.find((t: any) => t.test_id === t1.id).best_percent === 87 && row.tests.find((t: any) => t.test_id === t1.id).attempts === 3 && row.tests.find((t: any) => t.test_id === t1.id).attempts_left === 0, row);

  // ---- time limits ----
  const t3 = (await admin.post(`/training/courses/${course.id}/tests`, { title: 'Timed', time_limit_minutes: 1, show_answers: 'never' })).data;
  at = (await ann.post(`/training/tests/${t3.id}/attempts`)).data;
  ok('a timed attempt counts down', at.seconds_left > 50 && at.seconds_left <= 60 && at.deadline_at, at);
  at = (await ann.post(`/training/attempts/${at.id}/finish`)).data;
  ok('never showing answers', at.questions.every((x: any) => x.right_answers === null && x.explanation === null));
  ok('a test people took can\'t be deleted', (await admin.del(`/training/tests/${t3.id}`)).status === 400);

  // ---- assigning ----
  ok('assigning needs people', (await admin.post(`/training/courses/${course.id}/assign`, {})).status === 400);
  r = await admin.post(`/training/courses/${course.id}/assign`, { users: [bob.id, ann.id], due_on: tomorrow });
  ok('assigned to Bob (Ann already completed it)', r.data.assigned === 1 && r.data.already_completed === 1, r.data);
  r = await bob.get('/training/courses?filter=mine');
  const mine = r.data.items.find((c: any) => c.id === course.id);
  ok('Bob now has it, due tomorrow', mine && mine.enrolment.source === 'assigned' && mine.enrolment.due_on === tomorrow && mine.overdue === false, r.data);
  ok('he was told', (await bob.get('/notifications')).data.items.some((n: any) => n.title === `Training assigned: Selling ${stamp}`));
  ok('he can\'t leave it', (await bob.del(`/training/courses/${course.id}/enrol`)).status === 400);

  // ---- revoking ----
  await admin.del(`/training/certificates/${cert.id}`);
  r = await ann.get(`/training/courses/${course.id}`);
  ok('revoked: not completed any more', r.data.certificate === null && r.data.enrolment.completed_at === null, r.data);

  ok('only managers delete courses', (await ann.del(`/training/courses/${course.id}`)).status === 403);
  ok('deleted', (await admin.del(`/training/courses/${course.id}`)).status === 200 && (await admin.get(`/training/courses/${course.id}`)).status === 404);
} finally {
  await cleanupRun(stamp);
}
console.log(`\n${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
