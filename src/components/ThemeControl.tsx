import { useEffect, useState } from "react";
import { Monitor, Moon, Sun } from "lucide-react";
import { isDesktop } from "../api";

type ThemePreference = "light" | "dark" | "system";
const STORAGE_KEY = "filem:theme:v1";
const choices = [
  { value: "light", label: "浅色", Icon: Sun },
  { value: "dark", label: "暗色", Icon: Moon },
  { value: "system", label: "系统", Icon: Monitor },
] as const;

function preference(value: string | null | undefined): ThemePreference {
  return value === "light" || value === "dark" ? value : "system";
}

function applyTheme(value: ThemePreference) {
  const resolved =
    value === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : value;
  document.documentElement.dataset.themePreference = value;
  document.documentElement.dataset.theme = resolved;
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", resolved === "dark" ? "#181e21" : "#f5f7f6");
}

export function ThemeControl() {
  const [theme, setTheme] = useState<ThemePreference>(() =>
    preference(document.documentElement.dataset.themePreference),
  );

  useEffect(() => {
    applyTheme(theme);
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onSystemChange = () => applyTheme(theme);
    const onStorage = (event: StorageEvent) => {
      if (event.key === STORAGE_KEY || event.key === null) {
        setTheme(preference(event.newValue));
      }
    };
    if (theme === "system") media.addEventListener("change", onSystemChange);
    window.addEventListener("storage", onStorage);

    let cancelled = false;
    if (isDesktop()) {
      void import("@tauri-apps/api/app")
        .then(({ setTheme }) => {
          if (!cancelled) return setTheme(theme === "system" ? null : theme);
        })
        .catch((error: unknown) => {
          if (!cancelled) console.warn("系统窗口主题未能更新：", error);
        });
    }
    return () => {
      cancelled = true;
      media.removeEventListener("change", onSystemChange);
      window.removeEventListener("storage", onStorage);
    };
  }, [theme]);

  function choose(value: ThemePreference) {
    applyTheme(value);
    setTheme(value);
    try {
      localStorage.setItem(STORAGE_KEY, value);
    } catch {
      // Keep the selected appearance even if persistence is unavailable.
    }
  }

  return (
    <div className="theme-control">
      <div className="theme-options" role="group" aria-label="外观">
        {choices.map(({ value, label, Icon }) => (
          <button
            key={value}
            type="button"
            aria-label={value === "system" ? "跟随系统主题" : `${label}主题`}
            aria-pressed={theme === value}
            title={
              value === "system" ? "跟随系统的浅色或暗色设置" : `${label}主题`
            }
            onClick={() => choose(value)}
          >
            <Icon size={14} aria-hidden="true" />
            {label}
          </button>
        ))}
      </div>
    </div>
  );
}
