import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@xyflow/react/dist/style.css";
import "./styles/globals.css";
import "./lib/i18n";
import { router } from "./router";
import { AuthProvider } from "./lib/session";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 3_000,
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

const root = document.getElementById("root");

if (!root) {
  throw new Error("missing #root");
}

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <RouterProvider router={router} />
      </AuthProvider>
    </QueryClientProvider>
  </StrictMode>,
);
