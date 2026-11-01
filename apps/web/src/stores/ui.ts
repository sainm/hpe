import { create } from "zustand";

export type Theme = "dark" | "light" | "ocean";
export type ToastTone = "success" | "error" | "warning" | "info";

export interface Toast {
  id: number;
  message: string;
  tone: ToastTone;
}

interface UiState {
  theme: Theme;
  toasts: Toast[];
  setTheme: (theme: Theme) => void;
  pushToast: (message: string, tone?: ToastTone) => void;
  dismissToast: (id: number) => void;
}

const storedTheme = typeof window !== "undefined"
  ? window.localStorage.getItem("hpa.theme")
  : null;

const initialTheme: Theme =
  storedTheme === "light" || storedTheme === "ocean" || storedTheme === "dark"
    ? storedTheme
    : "light";

let toastId = 0;

export const useUiStore = create<UiState>((set) => ({
  theme: initialTheme,
  toasts: [],
  setTheme: (theme) => {
    window.localStorage.setItem("hpa.theme", theme);
    document.documentElement.dataset.theme = theme;
    set({ theme });
  },
  pushToast: (message, tone = "info") => {
    toastId += 1;
    const id = toastId;
    set((state) => ({ toasts: [...state.toasts, { id, message, tone }] }));
    window.setTimeout(() => {
      set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) }));
    }, 4200);
  },
  dismissToast: (id) =>
    set((state) => ({ toasts: state.toasts.filter((toast) => toast.id !== id) })),
}));

if (typeof document !== "undefined") {
  document.documentElement.dataset.theme = initialTheme;
}
