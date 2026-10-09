#!/usr/bin/env bash
#
# Nuke all Kobbler-written sidecar files from media root directories
# and clear sidecar metadata from the database.
#
# Usage: ./scripts/nuke-sidecars.sh <media_root_path> [<media_root_path2> ...]
#   or:  ./scripts/nuke-sidecars.sh --all-roots (reads paths from DB)
#
# What this deletes on disk:
#   - *.nfo (episode/movie/tvshow NFO files)
#   - metadata.json (Kobbler metadata sidecars)
#   - metadata.opf (book metadata sidecars)
#
# What this does NOT delete:
#   - Actual media files (mkv, mp4, epub, m4b, etc.)
#   - Image files (poster.jpg, fanart.jpg, etc.) — these come from the
#     filesystem, not from Kobbler writes
#
# After running, you should:
#   1. Clear DB sidecar metadata:
#      psql $DATABASE_URL -c "UPDATE media_root_files SET sidecar_metadata = NULL"
#   2. Re-scan media roots (or wait for the 5-minute scan check)
#   3. Re-add files to library (they'll get clean sidecars from enrichment)

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SIDECAR_PATTERNS=(
    "*.nfo"
    "metadata.json"
    "metadata.opf"
)

DRY_RUN=false
PATHS=()

for arg in "$@"; do
    case "$arg" in
        --dry-run)
            DRY_RUN=true
            ;;
        *)
            PATHS+=("$arg")
            ;;
    esac
done

if [ ${#PATHS[@]} -eq 0 ]; then
    echo -e "${RED}Usage: $0 [--dry-run] <media_root_path> [<media_root_path2> ...]${NC}"
    echo ""
    echo "Examples:"
    echo "  $0 /mnt/media/TV\\ Shows /mnt/media/Movies"
    echo "  $0 --dry-run /mnt/media/TV\\ Shows   # preview what would be deleted"
    exit 1
fi

total_deleted=0
total_found=0

for root_path in "${PATHS[@]}"; do
    if [ ! -d "$root_path" ]; then
        echo -e "${RED}Directory not found: $root_path${NC}"
        continue
    fi

    echo -e "${GREEN}Scanning: $root_path${NC}"

    for pattern in "${SIDECAR_PATTERNS[@]}"; do
        while IFS= read -r -d '' file; do
            total_found=$((total_found + 1))
            if [ "$DRY_RUN" = true ]; then
                echo "  [dry-run] would delete: $file"
            else
                rm -f "$file"
                total_deleted=$((total_deleted + 1))
            fi
        done < <(find "$root_path" -type f -name "$pattern" -print0 2>/dev/null)
    done
done

echo ""
if [ "$DRY_RUN" = true ]; then
    echo -e "${YELLOW}Dry run complete. Found $total_found sidecar files.${NC}"
    echo "Run without --dry-run to delete them."
else
    echo -e "${GREEN}Deleted $total_deleted sidecar files.${NC}"
    echo ""
    echo -e "${YELLOW}Next steps:${NC}"
    echo "  1. Clear DB sidecar metadata:"
    echo "     psql \$DATABASE_URL -c \"UPDATE media_root_files SET sidecar_metadata = NULL\""
    echo "  2. Re-scan media roots (happens automatically every 5 minutes, or trigger manually)"
fi
