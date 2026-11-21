/** Subset of the host get_world_snapshot response consumed by this plugin. */
export interface WorldSnapshotResponse {
  snapshot?: {
    hour: number;
    sunrise_sunset?: { sunrise_hour: number | null; sunset_hour: number | null } | null;
    weather?: { weather_code?: number | null; sunrise_hour?: number | null; sunset_hour?: number | null } | null;
  } | null;
}
