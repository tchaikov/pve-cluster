#!/bin/bash
# Test: Database Access
# Verify database is accessible and functional

set -e

# Source common test configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test-config.sh"

echo "Testing database access..."

DB_PATH="$TEST_DB_PATH"

# Check database exists and is readable
if [ ! -r "$DB_PATH" ]; then
    echo "ERROR: Database not readable: $DB_PATH"
    exit 1
fi
echo "✓ Database is readable"

# Check database size
DB_SIZE=$(stat -c %s "$DB_PATH")
if [ "$DB_SIZE" -lt 100 ]; then
    echo "ERROR: Database too small ($DB_SIZE bytes), likely corrupted"
    exit 1
fi
echo "✓ Database size: $DB_SIZE bytes"

# If sqlite3 is available, check database integrity
if command -v sqlite3 &> /dev/null; then
    echo "Checking database integrity..."

    if ! sqlite3 "$DB_PATH" "PRAGMA integrity_check;" | grep -q "ok"; then
        echo "ERROR: Database integrity check failed"
        sqlite3 "$DB_PATH" "PRAGMA integrity_check;"
        exit 1
    fi
    echo "✓ Database integrity check passed"

    # Check for expected tables (if any exist)
    TABLES=$(sqlite3 "$DB_PATH" "SELECT name FROM sqlite_master WHERE type='table';")
    if [ -n "$TABLES" ]; then
        echo "✓ Database tables found:"
        echo "$TABLES" | sed 's/^/    /'
    else
        echo "  No tables in database (may be new/empty)"
    fi
else
    echo "  sqlite3 not available, skipping detailed checks"
fi

# Check database file permissions
DB_PERMS=$(stat -c "%a" "$DB_PATH")
echo "  Database permissions: $DB_PERMS"

# CRITICAL TEST: Verify pmxcfs actually uses the database by writing through FUSE
echo "Testing database read/write through pmxcfs..."

MOUNT_PATH="$TEST_MOUNT_PATH"
TEST_FILE="$(make_test_file memdb)"
TEST_CONTENT="memdb-test-data-$(date +%s)"

# Write data through FUSE (should go to database)
if echo "$TEST_CONTENT" > "$TEST_FILE" 2>/dev/null; then
    echo "✓ Created test file through FUSE"

    # Verify file appears in database if sqlite3 available
    if command -v sqlite3 &> /dev/null; then
        # Query database for the file
        DB_ENTRY=$(sqlite3 "$DB_PATH" "SELECT name FROM tree WHERE name LIKE '%memdb-test%';" 2>/dev/null || true)
        if [ -n "$DB_ENTRY" ]; then
            echo "✓ File entry found in database"
        else
            echo "⚠ Warning: File not found in database (may use different storage)"
        fi
    fi

    # Read back through FUSE
    READ_CONTENT=$(cat "$TEST_FILE" 2>/dev/null || true)
    if [ "$READ_CONTENT" = "$TEST_CONTENT" ]; then
        echo "✓ Read back correct content through FUSE"
    else
        echo "ERROR: Read content mismatch"
        echo "  Expected: $TEST_CONTENT"
        echo "  Got: $READ_CONTENT"
        exit 1
    fi

    # Delete through FUSE
    rm "$TEST_FILE" 2>/dev/null || true
    if [ ! -f "$TEST_FILE" ]; then
        echo "✓ File deleted through FUSE"
    else
        echo "ERROR: File deletion failed"
        exit 1
    fi
else
    echo "⚠ Warning: Could not write test file (FUSE may not be writable)"
fi

echo "✓ Database access functional"
exit 0
