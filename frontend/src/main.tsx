import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router-dom";
import "./index.css";
import { router } from "./routes";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Audiobook + job state moves fast during generation. We still prefer
      // cached data on navigation, but always refetch on focus so an open
      // tab catches up after the user comes back.
      refetchOnWindowFocus: true,
      staleTime: 5_000,
      retry: (failureCount, err) => {
        // Don't burn retries on 4xx — only transient infra issues.
        const status = (err as { status?: number })?.status;
        if (status && status >= 400 && status < 500) return false;
        return failureCount < 2;
      },
    },
  },
});

const rootEl = document.getElementById("root");
if (!rootEl) throw new Error("missing #root");

createRoot(rootEl).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);
