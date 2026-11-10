// Removes everything test runs left behind (see cleanup.ts for how test data is recognised),
// and empties the local test mail accounts (sales@ and support@nerp.test), whose messages
// would otherwise come back as leads.
//
//   node cleanup-test-data.ts          show what would be removed
//   node cleanup-test-data.ts --yes    remove it
import { cleanup } from './cleanup.ts';
import { clearTestMail, TEST_MAILBOXES } from '../e2e/cleanup.ts';

const yes = process.argv.includes('--yes');
if (yes) clearTestMail(null);
const report = await cleanup({ yes });
report.push(`  all  messages in the test mailboxes (${TEST_MAILBOXES.join(', ')})`);
console.log(`${yes ? 'Removed' : 'Would remove (run with --yes)'}:\n${report.join('\n')}`);
