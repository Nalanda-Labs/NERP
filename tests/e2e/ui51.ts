// Training in the browser: a manager makes a course (details, a chapter and lesson, a question,
// a test); a learner enrols, reads lessons with a self-check, takes the test (choice, order and
// a written answer), the manager grades it, and the learner gets the certificate.
import { chromium } from 'playwright';
import { Client } from './lib.ts';
import { cleanupRun } from './cleanup.ts';
const B = 'http://localhost:8080';
const exe = process.env.HOME + '/.cache/ms-playwright/chromium_headless_shell-1254/chrome-headless-shell-linux64/chrome-headless-shell';
const browser = await chromium.launch({ executablePath: exe });
const errors: string[] = [];
const s = Date.now().toString(36);
let n = 0, failed = 0;
const pages: import('playwright').Page[] = [];
const step = async (name: string, fn: () => Promise<void>) => {
  try { await fn(); console.log('ok  ', name); } catch (e: any) { failed++; console.log('FAIL', name, e.message.split('\n').slice(0, 6).join(' | ')); ++n; for (const [i, p] of pages.entries()) await p.screenshot({ path: `fail51-${n}-${i}.png`, fullPage: true }); }
};
const admin = new Client();
await admin.post('/auth/login', { email: 'admin@acme.test', password: 'supersecret1' });
await admin.post('/users', { email: `ann.${s}@acme.test`, password: 'password123', first_name: 'Ann', last_name: 'Trainee' });

const ctx = async (email: string, password: string) => {
  const p = await (await browser.newContext({ viewport: { width: 1440, height: 1000 }, timezoneId: 'UTC', locale: 'en-GB' })).newPage();
  p.on('pageerror', (e) => errors.push('PAGEERROR ' + e.message));
  p.on('console', (m) => m.type() === 'error' && !/401|400|404|409|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  pages.push(p);
  await p.goto(B + '/login'); await p.fill('#email', email); await p.fill('#password', password);
  await p.click('button:has-text("Sign in")'); await p.waitForURL(B + '/');
  return p;
};
const course = `Onboarding ${s}`;
let courseId = '';

try {
  const ada = await ctx('admin@acme.test', 'supersecret1');

  await step('a manager makes a course and publishes it', async () => {
    await ada.goto(B + '/training');
    await ada.click('button:has-text("New course")');
    await ada.fill('#nc-title', course);
    await ada.click('button[form="new-course"]');
    await ada.waitForURL(/\/training\/courses\/[0-9a-f-]+\/edit$/);
    courseId = ada.url().split('/').at(-2)!;
    await ada.fill('#c-desc', 'Your **first week** here.');
    await ada.check('label:has-text("Published") input');
    await ada.click('button:has-text("Save")');
    await ada.locator('[data-published="true"]').waitFor();
  });

  await step('a chapter and a lesson with text', async () => {
    await ada.click('[role=tab]:has-text("Lessons")');
    await ada.fill('input[aria-label="New chapter"]', 'Week one');
    await ada.click('button:has-text("Add chapter")');
    await ada.locator('[data-chapter="Week one"]').waitFor();
    ada.once('dialog', (d) => d.accept('Meet the team'));
    await ada.click('[data-chapter="Week one"] button:has-text("Add a lesson")');
    await ada.waitForURL(/\/training\/lessons\/[0-9a-f-]+\?edit=1$/);
    await ada.fill('#ls-body', 'Say hello to **everyone**.');
    await ada.locator('.prose-kb strong:has-text("everyone")').waitFor();
    await ada.click('button:has-text("Save")');
    await ada.locator('[data-lesson-body] strong:has-text("everyone")').waitFor();
  });

  await step('questions: a self-check, a choice, an order and a written one', async () => {
    const lessons = (await admin.get(`/training/courses/${courseId}`)).data.lessons;
    const lesson = lessons[0].id;
    await admin.post(`/training/courses/${courseId}/lessons`, { title: 'Tools', chapter_id: (await admin.get(`/training/courses/${courseId}`)).data.chapters[0].id, body: 'We use NERP.' });
    await admin.post(`/training/courses/${courseId}/questions`, { kind: 'single', text: 'Who do you greet?', lesson_id: lesson, self_check: true, answers: [{ text: 'Everyone', is_correct: true }, { text: 'Nobody' }], explanation: 'Greet everyone.' });
    await admin.post(`/training/courses/${courseId}/questions`, { kind: 'order', text: 'Order your first day', points: 10, answers: [{ text: 'Arrive' }, { text: 'Coffee' }, { text: 'Work' }] });
    await admin.post(`/training/courses/${courseId}/questions`, { kind: 'text', text: 'What will you learn first?', points: 10 });
    await ada.goto(`${B}/training/courses/${courseId}/edit`);
    await ada.click('[role=tab]:has-text("Questions")');
    await ada.click('button:has-text("Add a question")');
    await ada.fill('#q-text', 'Where is the kitchen?');
    await ada.fill('dialog[open] input[aria-label="Answer 1"]', 'Second floor');
    await ada.fill('dialog[open] input[aria-label="Answer 2"]', 'Basement');
    await ada.click('button[form="q-form"]');
    await ada.locator('[data-question="Where is the kitchen?"]').waitFor();
  });

  await step('a test with marks', async () => {
    await ada.click('[role=tab]:has-text("Tests")');
    await ada.click('button:has-text("Add a test")');
    await ada.fill('#t-title', 'Week one check');
    await ada.uncheck('dialog[open] label:has-text("Shuffle questions") input');
    await ada.fill('#t-pass', '60');
    await ada.click('dialog[open] button:has-text("Add a mark")');
    await ada.fill('dialog[open] input[aria-label="Mark 1"]', 'Great');
    await ada.click('button[form="t-form"]');
    await ada.locator('[data-test="Week one check"]:has-text("3 questions")').waitFor();
  });

  const ann = await ctx(`ann.${s}@acme.test`, 'password123');

  await step('a learner finds the course and enrols', async () => {
    await ann.goto(B + '/training');
    await ann.click('[role=tab]:has-text("All courses")');
    await ann.click(`[data-course="${course}"]`);
    await ann.locator('[data-enrolment] button:has-text("Enrol")').click();
    await ann.locator('[data-enrolment]:has-text("0 of 2 lessons")').waitFor();
  });

  await step('reads the first lesson and checks herself', async () => {
    await ann.click('[data-lesson="Meet the team"] a');
    await ann.locator('[data-lesson-body] strong:has-text("everyone")').waitFor();
    await ann.check('[data-self-check] label:has-text("Nobody") input');
    await ann.click('[data-self-check] button:has-text("Check")');
    await ann.locator('[data-result="wrong"]:has-text("Greet everyone.")').waitFor();
    await ann.check('[data-self-check] label:has-text("Everyone") input');
    await ann.click('[data-self-check] button:has-text("Check")');
    await ann.locator('[data-result="right"]').waitFor();
    await ann.click('button:has-text("Done, next lesson")');
    await ann.locator('h1:has-text("Tools")').waitFor();
    await ann.click('button:text-is("Done")');
    await ann.waitForURL(`${B}/training/courses/${courseId}`);
    await ann.locator('[data-lesson="Tools"] [aria-label="done"]').waitFor();
  });

  await step('takes the test: choice, order and a written answer', async () => {
    await ann.click('[data-test="Week one check"] button:has-text("Start")');
    await ann.waitForURL(/\/training\/attempts\//);
    for (let i = 0; i < 3; i++) {
      await ann.click(`nav[aria-label="Questions"] button:has-text("${i + 1}")`);
      const sec = ann.locator('[data-question]');
      const text = await sec.innerText();
      if (text.includes('Order your first day')) {
        // Move "Arrive" to the top, then "Coffee" second.
        for (const [item, pos] of [['Arrive', 0], ['Coffee', 1]] as const) {
          for (let k = 0; k < 3; k++) {
            const items = await sec.locator('[data-item]').evaluateAll((els) => els.map((e) => e.getAttribute('data-item')));
            if (items.indexOf(item) <= pos) break;
            await sec.locator(`button[aria-label="Move up: ${item}"]`).click();
          }
        }
      } else if (text.includes('What will you learn first')) {
        await sec.locator('textarea').fill('Where everything is.');
      } else {
        await sec.locator('label:has-text("Second floor") input').check();
      }
      await sec.locator('button:has-text("Answer")').click();
      await ann.locator(`nav[aria-label="Questions"] button[aria-label="Question ${i + 1}, answered"]`).waitFor();
    }
    ann.once('dialog', (d) => d.accept());
    await ann.click('button:has-text("Finish the test")');
    await ann.locator('[data-result="review"]').waitFor();
  });

  await step('the manager grades the written answer', async () => {
    await ada.goto(B + '/training/reviews');
    const card = ada.locator('[data-review="Ann Trainee"]');
    await card.locator('[data-written]:has-text("Where everything is.")').waitFor();
    await card.locator('button:has-text("Right")').click();
    await card.waitFor({ state: 'detached' });
  });

  await step('the learner sees her result and gets the certificate', async () => {
    await ann.reload();
    await ann.locator('[data-percent]:has-text("100 %")').waitFor();
    await ann.locator('[data-passed="true"]:has-text("Great")').waitFor();
    await ann.click('a:has-text("Back to the course")');
    await ann.click('[data-enrolment] a:has-text("Certificate")');
    await ann.locator('[data-certificate] [data-name]:has-text("Ann Trainee")').waitFor();
    await ann.locator(`[data-certificate] [data-course]:has-text("${course}")`).waitFor();
  });

  await step('the gradebook shows her completed', async () => {
    await ada.goto(`${B}/training/courses/${courseId}/gradebook`);
    await ada.locator('tr[data-person="Ann Trainee"] a:has-text("100 % ✓")').waitFor();
    await ada.locator('tr[data-person="Ann Trainee"] a:has-text("No.")').waitFor();
  });
} finally {
  await browser.close();
  await cleanupRun(s);
}
console.log(errors.length ? 'console errors:\n' + errors.join('\n') : 'no console errors');
process.exit(failed || errors.length ? 1 : 0);
