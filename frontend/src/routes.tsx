import { createBrowserRouter, Navigate } from "react-router-dom";
import { AdminLayout } from "./components/AdminLayout";
import { AppLayout } from "./components/AppLayout";
import { RequireAdmin } from "./components/RequireAdmin";
import { RequireAuth } from "./components/RequireAuth";
import { AdminImageLlms } from "./pages/admin/AdminImageLlms";
import { AdminJobs } from "./pages/admin/AdminJobs";
import { AdminLlms } from "./pages/admin/AdminLlms";
import { AdminOverview } from "./pages/admin/AdminOverview";
import { AdminTestLlm } from "./pages/admin/AdminTestLlm";
import { AdminTestVoice } from "./pages/admin/AdminTestVoice";
import { AdminTopicTemplates } from "./pages/admin/AdminTopicTemplates";
import { AdminUsers } from "./pages/admin/AdminUsers";
import { AdminVoices } from "./pages/admin/AdminVoices";
import { AdminYoutubeSettings } from "./pages/admin/AdminYoutubeSettings";
import { BookDetail } from "./pages/BookDetail";
import { Library } from "./pages/Library";
import { Login } from "./pages/Login";
import { NewAudiobook } from "./pages/NewAudiobook";
import { Player } from "./pages/Player";
import { Settings } from "./pages/Settings";
import { Signup } from "./pages/Signup";

export const router = createBrowserRouter([
  { path: "/", element: <Navigate to="/app" replace /> },
  { path: "/login", element: <Login /> },
  { path: "/signup", element: <Signup /> },
  {
    element: (
      <RequireAuth>
        <AppLayout />
      </RequireAuth>
    ),
    children: [
      { path: "/app", element: <Library /> },
      { path: "/app/new", element: <NewAudiobook /> },
      { path: "/app/book/:id", element: <BookDetail /> },
      { path: "/app/play/:id", element: <Player /> },
      { path: "/app/settings", element: <Settings /> },
      {
        element: (
          <RequireAdmin>
            <AdminLayout />
          </RequireAdmin>
        ),
        children: [
          { path: "/admin", element: <AdminOverview /> },
          { path: "/admin/llm", element: <AdminLlms /> },
          { path: "/admin/image-llm", element: <AdminImageLlms /> },
          { path: "/admin/voice", element: <AdminVoices /> },
          { path: "/admin/topic-templates", element: <AdminTopicTemplates /> },
          { path: "/admin/youtube-settings", element: <AdminYoutubeSettings /> },
          { path: "/admin/test-llm", element: <AdminTestLlm /> },
          { path: "/admin/test-voice", element: <AdminTestVoice /> },
          { path: "/admin/users", element: <AdminUsers /> },
          { path: "/admin/jobs", element: <AdminJobs /> },
        ],
      },
    ],
  },
  { path: "*", element: <Navigate to="/app" replace /> },
]);
