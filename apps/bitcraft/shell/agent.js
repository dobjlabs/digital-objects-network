'use strict';

function showGuide() {
  const guide = window.bitcraftAgentGuide;
  const error = document.getElementById('agent-error');
  error.textContent = guide?.error ?? '';
  error.hidden = !error.textContent;
  document.getElementById('prompt').value = guide?.prompt ?? '';
  document.getElementById('copy').disabled = !guide?.prompt;
  document.getElementById('copy-status').textContent = '';
}

window.addEventListener('bitcraft-agent-guide-updated', showGuide);
showGuide();

document.getElementById('copy').addEventListener('click', async () => {
  const prompt = document.getElementById('prompt');
  const status = document.getElementById('copy-status');
  try {
    try {
      await navigator.clipboard.writeText(prompt.value);
    } catch (_) {
      prompt.focus();
      prompt.select();
      if (!document.execCommand('copy')) throw new Error('Clipboard unavailable');
    }
    status.textContent = 'Copied';
  } catch (_) {
    prompt.focus();
    prompt.select();
    status.textContent = 'Select the prompt and copy it manually.';
  }
});
