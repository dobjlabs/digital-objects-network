'use strict';
const guide = window.bitcraftAgent;
if (guide) {
  document.getElementById('prompt').value = guide.prompt;
  document.getElementById('skill-path').textContent = guide.skillPath;
  document.getElementById('enable-mcp').textContent = guide.enableMcp;
  document.getElementById('mcp-config').textContent = guide.mcpConfig;
}
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
