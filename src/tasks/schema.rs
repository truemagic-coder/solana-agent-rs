diesel::table! {
    scheduled_tasks (id) {
        id -> Integer,
        user_id -> Text,
        name -> Text,
        prompt -> Text,
        run_at -> BigInt,
        interval_minutes -> Nullable<BigInt>,
        enabled -> Bool,
        created_at -> BigInt,
        updated_at -> BigInt,
        last_run_at -> Nullable<BigInt>,
        next_run_at -> BigInt,
    }
}
