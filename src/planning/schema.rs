diesel::table! {
    plans (id) {
        id -> Integer,
        user_id -> Text,
        title -> Text,
        goal -> Text,
        steps_json -> Nullable<Text>,
        status -> Text,
        created_at -> BigInt,
        updated_at -> BigInt,
    }
}
