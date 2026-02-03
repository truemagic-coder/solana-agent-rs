diesel::table! {
    todo_items (id) {
        id -> Integer,
        user_id -> Text,
        title -> Text,
        notes -> Nullable<Text>,
        position -> Integer,
        created_at -> BigInt,
        updated_at -> BigInt,
        completed_at -> Nullable<BigInt>,
    }
}
