// Evaluate spoken content, not JSON punctuation or private metadata.
// These checks detect regressions; they do not measure empathy or naturalness.
export function evaluatePromptReply(reply, expect = {}) {
  const violations = [];
  let text = reply.trim();
  let payload;
  try { payload = JSON.parse(text); } catch { /* Plain text is valid for some modes. */ }
  if (expect.format === 'json') {
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
      violations.push('expected one JSON response object');
    } else {
      if (typeof payload.text !== 'string') violations.push('text must be a string');
      if (!['reply', 'short_reply', 'no_reply'].includes(payload.intent)) violations.push('invalid reply intent');
      if (payload.intent === 'no_reply' && payload.text !== '') violations.push('no_reply must have empty text');
    }
  }
  if (payload && typeof payload.text === 'string') text = payload.text.trim();
  if (expect.reply_language && text) {
    const latin = (text.match(/[A-Za-z]/g) ?? []).length;
    const han = (text.match(/\p{Script=Han}/gu) ?? []).length;
    const kana = (text.match(/[\u3040-\u30ff]/g) ?? []).length;
    const matches = expect.reply_language === 'en' ? latin > 0 && han + kana <= latin / 2
      : expect.reply_language === 'ja' ? kana > 0 && han + kana >= latin / 2
      : han > 0;
    if (!matches) violations.push(`reply language does not match ${expect.reply_language}`);
  }
  for (const pattern of expect.reply_forbidden ?? []) {
    if (new RegExp(pattern, 'i').test(text)) violations.push(`forbidden /${pattern}/`);
  }
  const allowedSilence = expect.allow_silence === true && text === '';
  if (!allowedSilence && expect.reply_required?.length && !expect.reply_required.some(pattern => new RegExp(pattern, 'i').test(text))) {
    violations.push(`none of required ${expect.reply_required.join(' | ')}`);
  }
  const chars = [...text].length;
  if (!allowedSilence && typeof expect.reply_min_chars === 'number' && chars < expect.reply_min_chars) {
    violations.push(`reply shorter than ${expect.reply_min_chars} chars (${chars})`);
  }
  if (typeof expect.reply_max_chars === 'number' && chars > expect.reply_max_chars) {
    violations.push(`reply longer than ${expect.reply_max_chars} chars (${chars})`);
  }
  // An observable punctuation limit, not a semantic question detector.
  const questions = (text.match(/[?？]/g) ?? []).length;
  if (typeof expect.reply_max_questions === 'number' && questions > expect.reply_max_questions) {
    violations.push(`more than ${expect.reply_max_questions} question marks (${questions})`);
  }
  if (expect.proactive_must_decline === true) {
    const declined = text === 'DONT_NOTIFY' || text === '';
    if (!declined) violations.push('did not decline on an unchanged repeated trigger');
  }
  return { text, violations };
}
