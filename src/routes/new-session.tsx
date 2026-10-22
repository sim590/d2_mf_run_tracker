import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";

import { NewSessionScreen } from "@/components/tracker/NewSessionScreen";
import { TrackerShell } from "@/components/tracker/TrackerShell";
import { useTrackerData } from "@/hooks/useTrackerData";
import { createSession } from "@/lib/tracker-db";
import { getTrackerErrorMessage } from "@/lib/tracker-error";

export const Route = createFileRoute("/new-session")({
  component: NewSessionRoute,
});

const POPULAR_AREA_NAMES = [
  "Mephisto",
  "Andariel",
  "Baal",
  "Diablo",
  "Pindleskin",
  "Ancient Tunnels",
  "Chaos Sanctuary",
  "Travincal",
  "Secret Cow Level",
  "The Pit",
  "Arcane Sanctuary",
  "Lower Kurast",
] as const;

function NewSessionRoute() {
  const navigate = useNavigate();
  const { activeSession, farmLocations, isLoading, loadError, refreshTracker } =
    useTrackerData();
  const [areaName, setAreaName] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [isStarting, setIsStarting] = useState(false);

  useEffect(() => {
    if (!isLoading && activeSession) {
      void navigate({ to: "/session", replace: true });
    }
  }, [activeSession, isLoading, navigate]);

  const popularAreas = useMemo(
    () =>
      POPULAR_AREA_NAMES.map(
        (name) =>
          farmLocations.find((location) => location.name === name)?.name ??
          name,
      ),
    [farmLocations],
  );

  const handleStartSession = async (nextAreaName: string) => {
    if (isStarting) {
      return;
    }

    const trimmedAreaName = nextAreaName.trim();

    if (!trimmedAreaName) {
      setFormError("Please enter a farming area before starting the session.");
      return;
    }

    setFormError(null);
    setIsStarting(true);

    try {
      await createSession(trimmedAreaName);
      await refreshTracker();
      void navigate({ to: "/session", replace: true });
    } catch (error) {
      setFormError(getTrackerErrorMessage(error));
    } finally {
      setIsStarting(false);
    }
  };

  return (
    <TrackerShell error={loadError}>
      <NewSessionScreen
        areaName={areaName}
        error={formError}
        isStarting={isStarting}
        popularAreas={popularAreas}
        onAreaNameChange={(value) => {
          setAreaName(value);
          if (formError) {
            setFormError(null);
          }
        }}
        onCancel={() => {
          void navigate({ to: "/" });
        }}
        onSelectArea={(value) => {
          setAreaName(value);
          setFormError(null);
        }}
        onStart={() => {
          void handleStartSession(areaName);
        }}
      />
    </TrackerShell>
  );
}
