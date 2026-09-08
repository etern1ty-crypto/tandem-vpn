export const STATE_LABELS = Object.freeze({
  running: "Работает", stopped: "Остановлена", start_pending: "Запускается",
  stop_pending: "Останавливается", continue_pending: "Возобновляется",
  pause_pending: "Приостанавливается", paused: "Приостановлена", unknown: "Неизвестно",
});
export function errorMessage(error) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  try { return JSON.stringify(error) ?? "Неизвестная ошибка"; } catch { return "Неизвестная ошибка"; }
}
export function validateBundle(tag, sha256) {
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(tag) || tag.includes("..") || !/\d/.test(tag)) throw new Error("Введите точный безопасный тег релиза.");
  if (!/^[a-fA-F0-9]{64}$/.test(sha256)) throw new Error("Нужна доверенная SHA-256: 64 hex-символа.");
  return { tag, sha256: sha256.toLowerCase() };
}
export function parseTargets(text) {
  const values = text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  if (values.length < 1 || values.length > 12) throw new Error("Укажите от 1 до 12 HTTPS-целей.");
  const seen = new Set();
  for (const value of values) {
    let url;
    try { url = new URL(value); } catch { throw new Error(`Некорректный адрес: ${value}`); }
    if (url.protocol !== "https:" || url.username || url.password || url.port || url.search || url.hash || !url.hostname.includes(".")) throw new Error("Цели должны быть HTTPS-адресами без учётных данных, нестандартных портов, query и fragment.");
    if (seen.has(url.href)) throw new Error("Уберите повторяющиеся цели.");
    seen.add(url.href);
  }
  return values;
}
export function appendLog(lines, text, timestamp, limit = 200) {
  return [...lines, `[${timestamp}] ${text}`].slice(-limit);
}
export function createRequestGate(onChange = () => {}) {
  let busy = false;
  return {
    get busy() { return busy; },
    async run(task) {
      if (busy) throw new Error("Дождитесь завершения текущей операции.");
      busy = true; onChange(true);
      try { return await task(); } finally { busy = false; onChange(false); }
    },
  };
}
