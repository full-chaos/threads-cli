use rusqlite::params;

pub(crate) fn test_only_count_edges_from(store: &crate::Store, from: &str) -> i64 {
    store
        .raw_conn()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE from_id = ?1 AND kind IN ('reply','root','mention','quote')",
            params![from],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
}

pub(crate) fn test_only_edge_target(
    store: &crate::Store,
    from: &str,
    kind: &str,
) -> Option<String> {
    store
        .raw_conn()
        .query_row(
            "SELECT to_id FROM edges WHERE from_id = ?1 AND kind = ?2",
            params![from, kind],
            |row| row.get::<_, String>(0),
        )
        .ok()
}
