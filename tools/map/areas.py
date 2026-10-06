"""Source-independent area lookup. Unknown memberships never act as empty sets."""
from dataclasses import dataclass
from pathlib import Path
import tomllib
from geometry import Grid, distance


class UnresolvedArea(ValueError):
    pass


class CampaignStateRequired(ValueError):
    pass


@dataclass(frozen=True)
class Members:
    hex_ids: tuple[str, ...]
    location_ids: tuple[str, ...]


class Areas:
    def __init__(self, folder: Path):
        self.grid=Grid(folder)
        self.data=tomllib.loads((folder/"areas.toml").read_text(encoding="utf-8"))
        if self.data["schema_version"] != 1 or self.data["build_file_sha256"] != self.grid.metadata["build_file_sha256"]:
            raise ValueError("Areas/grid provenance differs")
        self.areas={p["id"]:p for p in self.data["areas"]}
        self.locations={p["id"]:p for p in self.data["locations"]}

    def members(self, id):
        area=self.areas[id]
        if area["membership_status"] == "unresolved":
            raise UnresolvedArea(f"Unresolved area {id}: {area['reason']}")
        if area["membership_status"] == "requires_state":
            raise CampaignStateRequired(f"Area {id} needs campaign state")
        return Members(tuple(area["hex_ids"]),tuple(area["location_ids"]))

    def printed_location(self, token):
        matches=[p["id"] for p in self.locations.values() if p.get("printed_location")==token]
        if len(matches) != 1:
            raise ValueError(f"Printed location is missing or ambiguous: {token} ({matches})")
        return self.locations[matches[0]]

    def within(self, center, radius):
        if not isinstance(radius,int) or isinstance(radius,bool) or radius<0:
            raise ValueError("Radius must be a non-negative integer")
        origin=self.grid.to_axial(center)
        return tuple(sorted(name for name,row in self.grid.hexes.items()
                            if distance(origin,(int(row["q"]),int(row["r"])))<=radius))
