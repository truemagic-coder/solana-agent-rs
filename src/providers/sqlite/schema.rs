diesel::table! {
    messages (id) {
        id -> Integer,
        user_id -> Text,
        role -> Text,
        content -> Text,
        timestamp -> BigInt,
    }
}

diesel::table! {
    memories (id) {
        id -> Integer,
        user_id -> Text,
        summary -> Text,
        tags -> Nullable<Text>,
        salience -> Nullable<Double>,
        created_at -> BigInt,
    }
}

diesel::table! {
    entities (id) {
        id -> Integer,
        user_id -> Text,
        name -> Text,
        entity_type -> Text,
        canonical_id -> Nullable<Text>,
        created_at -> BigInt,
    }
}

diesel::table! {
    events (id) {
        id -> Integer,
        user_id -> Text,
        event_type -> Text,
        payload -> Nullable<Text>,
        occurred_at -> Nullable<BigInt>,
        created_at -> BigInt,
    }
}

diesel::table! {
    facts (id) {
        id -> Integer,
        user_id -> Text,
        subject -> Text,
        predicate -> Text,
        object -> Text,
        confidence -> Nullable<Double>,
        source -> Nullable<Text>,
        created_at -> BigInt,
    }
}

diesel::table! {
    edges (id) {
        id -> Integer,
        user_id -> Text,
        src_node_type -> Text,
        src_node_id -> Integer,
        dst_node_type -> Text,
        dst_node_id -> Integer,
        edge_type -> Text,
        weight -> Nullable<Double>,
        created_at -> BigInt,
    }
}

diesel::table! {
    memory_links (id) {
        id -> Integer,
        memory_id -> Integer,
        node_type -> Text,
        node_id -> Integer,
        created_at -> BigInt,
    }
}

diesel::table! {
    reminders (id) {
        id -> Integer,
        user_id -> Text,
        title -> Text,
        due_at -> BigInt,
        created_at -> BigInt,
        completed_at -> Nullable<BigInt>,
        fired_at -> Nullable<BigInt>,
    }
}
