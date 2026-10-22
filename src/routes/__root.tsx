import { createRootRoute, Outlet } from "@tanstack/react-router";
import { TanStackRouterDevtools } from "@tanstack/react-router-devtools";

import { AutoDetectBridge } from "@/components/auto-detect/AutoDetectBridge";

const RootLayout = () => (
  <>
    <AutoDetectBridge />
    <Outlet />
    <TanStackRouterDevtools position="bottom-right" />
  </>
);

export const Route = createRootRoute({ component: RootLayout });
