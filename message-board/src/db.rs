use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::types::{
    Claim, ClaimValidity, CreatePostRequest, CreateResponseRequest, Cursor, PostDto, ResponseDto,
};

#[derive(sqlx::FromRow)]
struct PostRow {
    id: Uuid,
    title: String,
    description: String,
    author_ip: String,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct PostClaimRow {
    post_id: Uuid,
    name: String,
    validity: String,
    hash: String,
    position: i32,
}

#[derive(sqlx::FromRow)]
struct ResponseRow {
    id: Uuid,
    post_id: Uuid,
    description: String,
    author_ip: String,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ResponseClaimRow {
    response_id: Uuid,
    name: String,
    validity: String,
    hash: String,
    position: i32,
}

fn validity_from_db(value: &str) -> ClaimValidity {
    match value {
        "nullified" => ClaimValidity::Nullified,
        _ => ClaimValidity::Live,
    }
}

pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn list_posts(
    pool: &PgPool,
    limit: u32,
    cursor: Option<Cursor>,
    search: Option<&str>,
    live_only: bool,
) -> Result<Vec<PostDto>> {
    let cursor_time = cursor.map(|value| value.created_at);
    let cursor_id = cursor.map(|value| value.id);

    let post_rows: Vec<PostRow> = sqlx::query_as::<_, PostRow>(
        r#"
        SELECT id, title, description, author_ip::text AS author_ip, created_at
        FROM posts p
        WHERE (
            $1::timestamptz IS NULL OR
            (p.created_at, p.id) < ($1, $2::uuid)
        )
        AND (
            $3::text IS NULL OR
            p.title ILIKE ('%' || $3 || '%') OR
            p.description ILIKE ('%' || $3 || '%')
        )
        AND (
            $4::boolean = false OR
            NOT EXISTS (
                SELECT 1
                FROM post_claims pc
                WHERE pc.post_id = p.id
                AND pc.validity <> 'live'
            )
        )
        ORDER BY p.created_at DESC, p.id DESC
        LIMIT $5
        "#,
    )
    .bind(cursor_time)
    .bind(cursor_id)
    .bind(search)
    .bind(live_only)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    if post_rows.is_empty() {
        return Ok(Vec::new());
    }

    let post_ids: Vec<Uuid> = post_rows.iter().map(|row| row.id).collect();

    let post_claim_rows: Vec<PostClaimRow> = sqlx::query_as::<_, PostClaimRow>(
        r#"
        SELECT post_id, name, validity, hash, position
        FROM post_claims
        WHERE post_id = ANY($1)
        ORDER BY post_id, position ASC
        "#,
    )
    .bind(&post_ids)
    .fetch_all(pool)
    .await?;

    let response_rows: Vec<ResponseRow> = sqlx::query_as::<_, ResponseRow>(
        r#"
        SELECT id, post_id, description, author_ip::text AS author_ip, created_at
        FROM responses
        WHERE post_id = ANY($1)
        ORDER BY post_id, created_at ASC, id ASC
        "#,
    )
    .bind(&post_ids)
    .fetch_all(pool)
    .await?;

    let response_ids: Vec<Uuid> = response_rows.iter().map(|row| row.id).collect();

    let response_claim_rows: Vec<ResponseClaimRow> = if response_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, ResponseClaimRow>(
            r#"
            SELECT response_id, name, validity, hash, position
            FROM response_claims
            WHERE response_id = ANY($1)
            ORDER BY response_id, position ASC
            "#,
        )
        .bind(&response_ids)
        .fetch_all(pool)
        .await?
    };

    let mut post_claims_map: HashMap<Uuid, Vec<(i32, Claim)>> = HashMap::new();
    for row in post_claim_rows {
        post_claims_map.entry(row.post_id).or_default().push((
            row.position,
            Claim {
                name: row.name,
                validity: validity_from_db(&row.validity),
                hash: row.hash,
            },
        ));
    }

    let mut response_claims_map: HashMap<Uuid, Vec<(i32, Claim)>> = HashMap::new();
    for row in response_claim_rows {
        response_claims_map
            .entry(row.response_id)
            .or_default()
            .push((
                row.position,
                Claim {
                    name: row.name,
                    validity: validity_from_db(&row.validity),
                    hash: row.hash,
                },
            ));
    }

    let mut responses_map: HashMap<Uuid, Vec<ResponseDto>> = HashMap::new();
    for row in response_rows {
        let mut proofs = response_claims_map.remove(&row.id).unwrap_or_default();
        proofs.sort_by_key(|(pos, _)| *pos);
        let proofs = proofs.into_iter().map(|(_, claim)| claim).collect();

        responses_map
            .entry(row.post_id)
            .or_default()
            .push(ResponseDto {
                id: row.id,
                post_id: row.post_id,
                peer: row.author_ip,
                time: row.created_at,
                desc: row.description,
                proofs,
            });
    }

    let mut items = Vec::with_capacity(post_rows.len());
    for row in post_rows {
        let mut proofs = post_claims_map.remove(&row.id).unwrap_or_default();
        proofs.sort_by_key(|(pos, _)| *pos);
        let proofs = proofs.into_iter().map(|(_, claim)| claim).collect();

        let responses = responses_map.remove(&row.id).unwrap_or_default();

        items.push(PostDto {
            id: row.id,
            title: row.title,
            description: row.description,
            peer: row.author_ip,
            time: row.created_at,
            proofs,
            responses,
        });
    }

    Ok(items)
}

pub async fn create_post(
    pool: &PgPool,
    author_ip: &str,
    request: CreatePostRequest,
) -> Result<PostDto> {
    let mut tx = pool.begin().await?;

    let post_id = Uuid::new_v4();
    let row: PostRow = sqlx::query_as::<_, PostRow>(
        r#"
        INSERT INTO posts (id, title, description, author_ip)
        VALUES ($1, $2, $3, $4::inet)
        RETURNING id, title, description, author_ip::text AS author_ip, created_at
        "#,
    )
    .bind(post_id)
    .bind(&request.title)
    .bind(&request.description)
    .bind(author_ip)
    .fetch_one(&mut *tx)
    .await?;

    for (position, claim) in request.claims.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO post_claims (id, post_id, name, validity, hash, position)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(post_id)
        .bind(&claim.name)
        .bind(claim.validity.as_db_str())
        .bind(&claim.hash)
        .bind(position as i32)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(PostDto {
        id: row.id,
        title: row.title,
        description: row.description,
        peer: row.author_ip,
        time: row.created_at,
        proofs: request.claims,
        responses: Vec::new(),
    })
}

pub async fn create_response(
    pool: &PgPool,
    post_id: Uuid,
    author_ip: &str,
    request: CreateResponseRequest,
) -> Result<Option<ResponseDto>> {
    let mut tx = pool.begin().await?;

    let post_exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(&mut *tx)
        .await?;

    if post_exists.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }

    let response_id = Uuid::new_v4();
    let row: ResponseRow = sqlx::query_as::<_, ResponseRow>(
        r#"
        INSERT INTO responses (id, post_id, description, author_ip)
        VALUES ($1, $2, $3, $4::inet)
        RETURNING id, post_id, description, author_ip::text AS author_ip, created_at
        "#,
    )
    .bind(response_id)
    .bind(post_id)
    .bind(&request.description)
    .bind(author_ip)
    .fetch_one(&mut *tx)
    .await?;

    for (position, claim) in request.claims.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO response_claims (id, response_id, name, validity, hash, position)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(response_id)
        .bind(&claim.name)
        .bind(claim.validity.as_db_str())
        .bind(&claim.hash)
        .bind(position as i32)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Some(ResponseDto {
        id: row.id,
        post_id: row.post_id,
        peer: row.author_ip,
        time: row.created_at,
        desc: row.description,
        proofs: request.claims,
    }))
}
