use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroUsize};

use mako_storage::{KeyRange, ScanDirection, ScanRequest, StorageError};
use serde::{Deserialize, Serialize};

use crate::{
    CanonicalDocument, DocumentCodecError, DocumentId, DocumentReadAuthorizer, IndexDefinition,
    IndexDirection, IndexError, IndexName, IndexState, IndexValue, IndexVersion,
    ReadAuthorizationContext, ReadAuthorizationPath, ScopedCollectionEngine,
    ScopedCollectionSnapshot, index::IndexCatalog,
};

const QUERY_CURSOR_ENCODING_VERSION: u8 = 1;
pub const MAX_TRUSTED_QUERY_LIMIT: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustedQuery {
    PrimaryKey(DocumentId),
    Indexed(IndexedQuery),
}

impl TrustedQuery {
    #[must_use]
    pub fn by_primary_key(document_id: DocumentId) -> Self {
        Self::PrimaryKey(document_id)
    }

    #[must_use]
    pub fn indexed(query: IndexedQuery) -> Self {
        Self::Indexed(query)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedQuery {
    predicates: Vec<QueryPredicate>,
    sort: Vec<QuerySort>,
    cursor: Option<QueryCursor>,
    limit: NonZeroUsize,
}

impl IndexedQuery {
    pub fn new(
        predicates: impl IntoIterator<Item = QueryPredicate>,
        sort: impl IntoIterator<Item = QuerySort>,
        limit: NonZeroUsize,
    ) -> Result<Self, TrustedQueryError> {
        if limit.get() > MAX_TRUSTED_QUERY_LIMIT {
            return Err(TrustedQueryError::LimitTooLarge {
                requested: limit.get(),
                maximum: MAX_TRUSTED_QUERY_LIMIT,
            });
        }
        Ok(Self {
            predicates: predicates.into_iter().collect(),
            sort: sort.into_iter().collect(),
            cursor: None,
            limit,
        })
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: QueryCursor) -> Self {
        self.cursor = Some(cursor);
        self
    }

    #[must_use]
    pub fn predicates(&self) -> &[QueryPredicate] {
        &self.predicates
    }

    #[must_use]
    pub fn sort(&self) -> &[QuerySort] {
        &self.sort
    }

    #[must_use]
    pub fn cursor(&self) -> Option<&QueryCursor> {
        self.cursor.as_ref()
    }

    #[must_use]
    pub const fn limit(&self) -> NonZeroUsize {
        self.limit
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryPredicate {
    field: String,
    operation: QueryPredicateOperation,
}

impl QueryPredicate {
    pub fn equal(field: impl Into<String>, value: IndexValue) -> Result<Self, TrustedQueryError> {
        let field = field.into();
        validate_query_field(&field)?;
        Ok(Self {
            field,
            operation: QueryPredicateOperation::Equal(value),
        })
    }

    pub fn bounded_range(
        field: impl Into<String>,
        lower: QueryBound,
        upper: QueryBound,
    ) -> Result<Self, TrustedQueryError> {
        let field = field.into();
        validate_query_field(&field)?;
        if lower.value.encode(IndexDirection::Ascending)
            > upper.value.encode(IndexDirection::Ascending)
        {
            return Err(TrustedQueryError::InvalidRangeBounds);
        }
        Ok(Self {
            field,
            operation: QueryPredicateOperation::Range {
                lower: Some(lower),
                upper: Some(upper),
            },
        })
    }

    pub fn greater_than(
        field: impl Into<String>,
        value: IndexValue,
    ) -> Result<Self, TrustedQueryError> {
        Self::one_sided_range(field, Some(QueryBound::exclusive(value)), None)
    }

    pub fn greater_than_or_equal(
        field: impl Into<String>,
        value: IndexValue,
    ) -> Result<Self, TrustedQueryError> {
        Self::one_sided_range(field, Some(QueryBound::inclusive(value)), None)
    }

    pub fn less_than(
        field: impl Into<String>,
        value: IndexValue,
    ) -> Result<Self, TrustedQueryError> {
        Self::one_sided_range(field, None, Some(QueryBound::exclusive(value)))
    }

    pub fn less_than_or_equal(
        field: impl Into<String>,
        value: IndexValue,
    ) -> Result<Self, TrustedQueryError> {
        Self::one_sided_range(field, None, Some(QueryBound::inclusive(value)))
    }

    fn one_sided_range(
        field: impl Into<String>,
        lower: Option<QueryBound>,
        upper: Option<QueryBound>,
    ) -> Result<Self, TrustedQueryError> {
        let field = field.into();
        validate_query_field(&field)?;
        Ok(Self {
            field,
            operation: QueryPredicateOperation::Range { lower, upper },
        })
    }

    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum QueryPredicateOperation {
    Equal(IndexValue),
    Range {
        lower: Option<QueryBound>,
        upper: Option<QueryBound>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryBound {
    value: IndexValue,
    inclusive: bool,
}

impl QueryBound {
    #[must_use]
    pub fn inclusive(value: IndexValue) -> Self {
        Self {
            value,
            inclusive: true,
        }
    }

    #[must_use]
    pub fn exclusive(value: IndexValue) -> Self {
        Self {
            value,
            inclusive: false,
        }
    }

    #[must_use]
    pub const fn is_inclusive(&self) -> bool {
        self.inclusive
    }

    #[must_use]
    pub fn into_value(self) -> IndexValue {
        self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuerySort {
    field: String,
    direction: IndexDirection,
}

impl QuerySort {
    pub fn new(
        field: impl Into<String>,
        direction: IndexDirection,
    ) -> Result<Self, TrustedQueryError> {
        let field = field.into();
        validate_query_field(&field)?;
        Ok(Self { field, direction })
    }

    pub fn ascending(field: impl Into<String>) -> Result<Self, TrustedQueryError> {
        Self::new(field, IndexDirection::Ascending)
    }

    pub fn descending(field: impl Into<String>) -> Result<Self, TrustedQueryError> {
        Self::new(field, IndexDirection::Descending)
    }

    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }

    #[must_use]
    pub const fn direction(&self) -> IndexDirection {
        self.direction
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryCursor(String);

impl QueryCursor {
    pub fn parse(value: impl Into<String>) -> Result<Self, TrustedQueryError> {
        let cursor = Self(value.into());
        if cursor.0.len() > 4_096 {
            return Err(TrustedQueryError::InvalidCursor);
        }
        decode_cursor(&cursor)?;
        Ok(cursor)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryPlan {
    PrimaryKey {
        document_id: DocumentId,
    },
    Index {
        name: IndexName,
        version: IndexVersion,
        equality_fields: usize,
        range_field: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueryPage {
    documents: Vec<CanonicalDocument>,
    next_cursor: Option<QueryCursor>,
}

impl QueryPage {
    #[must_use]
    pub fn documents(&self) -> &[CanonicalDocument] {
        &self.documents
    }

    #[must_use]
    pub fn next_cursor(&self) -> Option<&QueryCursor> {
        self.next_cursor.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequiredIndexShape {
    equality_fields: Vec<String>,
    range_field: Option<String>,
    sort: Vec<QuerySort>,
}

impl RequiredIndexShape {
    #[must_use]
    pub fn equality_fields(&self) -> &[String] {
        &self.equality_fields
    }

    #[must_use]
    pub fn range_field(&self) -> Option<&str> {
        self.range_field.as_deref()
    }

    #[must_use]
    pub fn sort(&self) -> &[QuerySort] {
        &self.sort
    }
}

impl ScopedCollectionEngine {
    pub async fn plan_trusted_query(
        &self,
        query: &TrustedQuery,
    ) -> Result<QueryPlan, TrustedQueryError> {
        match query {
            TrustedQuery::PrimaryKey(document_id) => Ok(QueryPlan::PrimaryKey {
                document_id: document_id.clone(),
            }),
            TrustedQuery::Indexed(query) => {
                let (_, _, catalog) = self.load_index_catalog().await?;
                let plan = plan_index_query(catalog.definitions(), query)?;
                Ok(plan.public())
            }
        }
    }

    pub async fn trusted_query(
        &self,
        query: &TrustedQuery,
    ) -> Result<QueryPage, TrustedQueryError> {
        self.trusted_query_inner(query, None).await
    }

    pub async fn trusted_query_authorized(
        &self,
        query: &TrustedQuery,
        authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<QueryPage, TrustedQueryError> {
        self.trusted_query_inner(query, Some(authorizer)).await
    }

    async fn trusted_query_inner(
        &self,
        query: &TrustedQuery,
        authorizer: Option<&dyn DocumentReadAuthorizer>,
    ) -> Result<QueryPage, TrustedQueryError> {
        let snapshot = self.adapter.snapshot().await?;
        self.trusted_query_on_snapshot(snapshot.as_ref(), query, authorizer)
            .await
    }

    async fn trusted_query_on_snapshot(
        &self,
        snapshot: &dyn mako_storage::KvSnapshot,
        query: &TrustedQuery,
        authorizer: Option<&dyn DocumentReadAuthorizer>,
    ) -> Result<QueryPage, TrustedQueryError> {
        match query {
            TrustedQuery::PrimaryKey(document_id) => {
                let key = self.document_key(document_id)?;
                let document = snapshot
                    .get(&key)
                    .await?
                    .as_deref()
                    .map(CanonicalDocument::decode)
                    .transpose()?
                    .filter(|document| !document.is_deleted())
                    .filter(|document| {
                        authorizer.is_none_or(|authorizer| {
                            authorizer
                                .authorize_read(ReadAuthorizationContext::new(
                                    self,
                                    ReadAuthorizationPath::TrustedQuery,
                                    document,
                                ))
                                .is_allowed()
                        })
                    });
                Ok(QueryPage {
                    documents: document.into_iter().collect(),
                    next_cursor: None,
                })
            }
            TrustedQuery::Indexed(query) => {
                let catalog_key = self.index_catalog_key()?;
                let catalog = snapshot
                    .get(&catalog_key)
                    .await?
                    .as_deref()
                    .map(IndexCatalog::decode)
                    .transpose()?
                    .unwrap_or_default();
                let plan = plan_index_query(catalog.definitions(), query)?;
                self.execute_index_plan(snapshot, query, &plan, authorizer)
                    .await
            }
        }
    }

    async fn execute_index_plan(
        &self,
        snapshot: &dyn mako_storage::KvSnapshot,
        query: &IndexedQuery,
        plan: &PlannedIndexQuery,
        authorizer: Option<&dyn DocumentReadAuthorizer>,
    ) -> Result<QueryPage, TrustedQueryError> {
        let physical_name = plan.definition.physical_name();
        let mut range = self.index_query_range(plan)?;
        if let Some(cursor) = query.cursor() {
            let decoded = decode_cursor(cursor)?;
            if decoded.physical_name != physical_name || !range.contains(&decoded.key) {
                return Err(TrustedQueryError::InvalidCursor);
            }
            let mut after = decoded.key;
            after.push(0);
            if after >= range.end_exclusive {
                return Ok(QueryPage {
                    documents: Vec::new(),
                    next_cursor: None,
                });
            }
            range.start_inclusive = after;
        }

        let wanted = query.limit().get() + 1;
        let maximum_page = self.adapter.capabilities().maximum_scan_items.get();
        let mut results = Vec::with_capacity(wanted);
        let mut scan_range = range;
        while results.len() < wanted {
            let scan_limit = NonZeroUsize::new((wanted - results.len()).min(maximum_page))
                .expect("wanted results are non-zero");
            let entries = snapshot
                .scan(ScanRequest::new(
                    scan_range.clone(),
                    ScanDirection::Forward,
                    scan_limit,
                ))
                .await?;
            let full = entries.len() == scan_limit.get();
            let Some(last_key) = entries.last().map(|entry| entry.key.clone()) else {
                break;
            };
            for entry in entries {
                let decoded = self.keyspace.decode_index_entry_key(
                    self.scope().collection_id().as_str(),
                    &physical_name,
                    &entry.key,
                )?;
                let document_id = String::from_utf8(decoded.document)
                    .map_err(|_| TrustedQueryError::CorruptIndexEntry)?;
                let document_key = self
                    .keyspace
                    .document_key(self.scope().collection_id().as_str(), document_id)?;
                if let Some(document) = snapshot
                    .get(&document_key)
                    .await?
                    .as_deref()
                    .map(CanonicalDocument::decode)
                    .transpose()?
                    .filter(|document| !document.is_deleted())
                    .filter(|document| {
                        authorizer.is_none_or(|authorizer| {
                            authorizer
                                .authorize_read(ReadAuthorizationContext::new(
                                    self,
                                    ReadAuthorizationPath::TrustedQuery,
                                    document,
                                ))
                                .is_allowed()
                        })
                    })
                {
                    results.push((entry.key, document));
                    if results.len() == wanted {
                        break;
                    }
                }
            }
            if results.len() == wanted || !full {
                break;
            }
            let mut next = last_key;
            next.push(0);
            if next >= scan_range.end_exclusive {
                break;
            }
            scan_range.start_inclusive = next;
        }

        let has_more = results.len() > query.limit().get();
        if has_more {
            results.truncate(query.limit().get());
        }
        let next_cursor = if has_more {
            results
                .last()
                .map(|(key, _)| encode_cursor(&physical_name, key))
                .transpose()?
        } else {
            None
        };
        Ok(QueryPage {
            documents: results.into_iter().map(|(_, document)| document).collect(),
            next_cursor,
        })
    }

    fn index_query_range(&self, plan: &PlannedIndexQuery) -> Result<KeyRange, TrustedQueryError> {
        let collection = self.scope().collection_id().as_str();
        let physical_name = plan.definition.physical_name();
        let equality_components: Vec<_> = plan
            .equality
            .iter()
            .zip(plan.definition.fields())
            .map(|(value, field)| value.encode(field.direction()))
            .collect();
        let equality_prefix = self.keyspace.index_components_prefix(
            collection,
            &physical_name,
            &equality_components,
        )?;
        let equality_range = mako_storage::TenantKeyspace::prefixed_range(&equality_prefix)?;
        let Some((range_index, lower, upper)) = &plan.range else {
            return Ok(equality_range);
        };

        let field = &plan.definition.fields()[*range_index];
        let (physical_lower, physical_upper) = if field.direction() == IndexDirection::Ascending {
            (lower.as_ref(), upper.as_ref())
        } else {
            (upper.as_ref(), lower.as_ref())
        };
        let start_inclusive = match physical_lower {
            Some(bound) => {
                let mut components = equality_components.clone();
                components.push(bound.value.encode(field.direction()));
                let prefix = self.keyspace.index_components_prefix(
                    collection,
                    &physical_name,
                    &components,
                )?;
                if bound.inclusive {
                    prefix
                } else {
                    mako_storage::TenantKeyspace::prefixed_range(&prefix)?.end_exclusive
                }
            }
            None => equality_range.start_inclusive,
        };
        let end_exclusive = match physical_upper {
            Some(bound) => {
                let mut components = equality_components;
                components.push(bound.value.encode(field.direction()));
                let prefix = self.keyspace.index_components_prefix(
                    collection,
                    physical_name,
                    &components,
                )?;
                if bound.inclusive {
                    mako_storage::TenantKeyspace::prefixed_range(&prefix)?.end_exclusive
                } else {
                    prefix
                }
            }
            None => equality_range.end_exclusive,
        };
        KeyRange::new(start_inclusive, end_exclusive)
            .map_err(|_| TrustedQueryError::InvalidRangeBounds)
    }
}

impl ScopedCollectionSnapshot {
    /// Execute a trusted query against this snapshot rather than opening a new
    /// one. Explorer continuations use this to keep every page on one stable
    /// read view.
    pub async fn trusted_query(
        &self,
        query: &TrustedQuery,
    ) -> Result<QueryPage, TrustedQueryError> {
        self.engine
            .trusted_query_on_snapshot(self.snapshot.as_ref(), query, None)
            .await
    }

    /// Execute an authorized trusted query against this stable snapshot.
    pub async fn trusted_query_authorized(
        &self,
        query: &TrustedQuery,
        authorizer: &dyn DocumentReadAuthorizer,
    ) -> Result<QueryPage, TrustedQueryError> {
        self.engine
            .trusted_query_on_snapshot(self.snapshot.as_ref(), query, Some(authorizer))
            .await
    }
}

#[derive(Clone, Debug)]
struct PlannedIndexQuery {
    definition: IndexDefinition,
    equality: Vec<IndexValue>,
    range: Option<(usize, Option<QueryBound>, Option<QueryBound>)>,
}

impl PlannedIndexQuery {
    fn public(&self) -> QueryPlan {
        QueryPlan::Index {
            name: self.definition.name().clone(),
            version: self.definition.version(),
            equality_fields: self.equality.len(),
            range_field: self
                .range
                .as_ref()
                .map(|(index, _, _)| self.definition.fields()[*index].path().to_owned()),
        }
    }
}

fn plan_index_query(
    definitions: &[IndexDefinition],
    query: &IndexedQuery,
) -> Result<PlannedIndexQuery, TrustedQueryError> {
    let shape = required_shape(query)?;
    let mut equality = BTreeMap::new();
    let mut range = None;
    for predicate in query.predicates() {
        match &predicate.operation {
            QueryPredicateOperation::Equal(value) => {
                equality.insert(predicate.field(), value.clone());
            }
            QueryPredicateOperation::Range { lower, upper } => {
                range = Some((predicate.field(), lower.clone(), upper.clone()));
            }
        }
    }

    let mut candidates = Vec::new();
    for definition in definitions
        .iter()
        .filter(|definition| definition.state() == IndexState::Active)
    {
        let mut ordered_equality = Vec::new();
        for field in definition.fields() {
            let Some(value) = equality.get(field.path()) else {
                break;
            };
            ordered_equality.push((*value).clone());
        }
        if ordered_equality.len() != equality.len() {
            continue;
        }
        let planned_range = match &range {
            Some((range_field, lower, upper)) => {
                let range_index = ordered_equality.len();
                if definition
                    .fields()
                    .get(range_index)
                    .map(|field| field.path())
                    != Some(*range_field)
                {
                    continue;
                }
                Some((range_index, lower.clone(), upper.clone()))
            }
            None => None,
        };
        let sort_start = ordered_equality.len();
        if query.sort().len() > definition.fields().len().saturating_sub(sort_start)
            || !query
                .sort()
                .iter()
                .zip(&definition.fields()[sort_start..])
                .all(|(requested, indexed)| {
                    requested.field() == indexed.path()
                        && requested.direction() == indexed.direction()
                })
        {
            continue;
        }
        candidates.push(PlannedIndexQuery {
            definition: definition.clone(),
            equality: ordered_equality,
            range: planned_range,
        });
    }
    candidates.sort_by(|left, right| {
        right
            .equality
            .len()
            .cmp(&left.equality.len())
            .then_with(|| {
                left.definition
                    .fields()
                    .len()
                    .cmp(&right.definition.fields().len())
            })
            .then_with(|| left.definition.name().cmp(right.definition.name()))
            .then_with(|| left.definition.version().cmp(&right.definition.version()))
    });
    candidates
        .into_iter()
        .next()
        .ok_or(TrustedQueryError::RequiredIndex { shape })
}

fn required_shape(query: &IndexedQuery) -> Result<RequiredIndexShape, TrustedQueryError> {
    if query.predicates().is_empty() {
        return Err(TrustedQueryError::RequiredIndex {
            shape: RequiredIndexShape {
                equality_fields: Vec::new(),
                range_field: None,
                sort: query.sort().to_vec(),
            },
        });
    }
    let mut equality_fields = Vec::new();
    let mut range_field = None;
    let mut seen = BTreeMap::new();
    for predicate in query.predicates() {
        if seen.insert(predicate.field(), ()).is_some() {
            return Err(TrustedQueryError::DuplicatePredicateField(
                predicate.field().to_owned(),
            ));
        }
        match &predicate.operation {
            QueryPredicateOperation::Equal(_) => equality_fields.push(predicate.field().to_owned()),
            QueryPredicateOperation::Range { .. } if range_field.is_none() => {
                range_field = Some(predicate.field().to_owned());
            }
            QueryPredicateOperation::Range { .. } => {
                return Err(TrustedQueryError::MultipleRangePredicates);
            }
        }
    }
    equality_fields.sort();
    Ok(RequiredIndexShape {
        equality_fields,
        range_field,
        sort: query.sort().to_vec(),
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryCursorWire {
    encoding_version: u8,
    physical_name: String,
    key_hex: String,
}

struct DecodedCursor {
    physical_name: String,
    key: Vec<u8>,
}

fn encode_cursor(physical_name: &str, key: &[u8]) -> Result<QueryCursor, TrustedQueryError> {
    let bytes = serde_json::to_vec(&QueryCursorWire {
        encoding_version: QUERY_CURSOR_ENCODING_VERSION,
        physical_name: physical_name.to_owned(),
        key_hex: hex_encode(key),
    })
    .map_err(TrustedQueryError::Json)?;
    Ok(QueryCursor(format!("q1_{}", hex_encode(&bytes))))
}

fn decode_cursor(cursor: &QueryCursor) -> Result<DecodedCursor, TrustedQueryError> {
    let encoded = cursor
        .as_str()
        .strip_prefix("q1_")
        .ok_or(TrustedQueryError::InvalidCursor)?;
    let wire: QueryCursorWire = serde_json::from_slice(&hex_decode(encoded)?)
        .map_err(|_| TrustedQueryError::InvalidCursor)?;
    if wire.encoding_version != QUERY_CURSOR_ENCODING_VERSION {
        return Err(TrustedQueryError::InvalidCursor);
    }
    Ok(DecodedCursor {
        physical_name: wire.physical_name,
        key: hex_decode(&wire.key_hex)?,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_decode(value: &str) -> Result<Vec<u8>, TrustedQueryError> {
    if !value.len().is_multiple_of(2) {
        return Err(TrustedQueryError::InvalidCursor);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Result<u8, TrustedQueryError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(TrustedQueryError::InvalidCursor),
    }
}

fn validate_query_field(field: &str) -> Result<(), TrustedQueryError> {
    crate::IndexField::ascending(field)
        .map(|_| ())
        .map_err(|_| TrustedQueryError::InvalidField(field.to_owned()))
}

#[derive(Debug)]
pub enum TrustedQueryError {
    RequiredIndex { shape: RequiredIndexShape },
    InvalidField(String),
    DuplicatePredicateField(String),
    MultipleRangePredicates,
    InvalidRangeBounds,
    LimitTooLarge { requested: usize, maximum: usize },
    InvalidCursor,
    CorruptIndexEntry,
    Index(IndexError),
    Scope(crate::DocumentEngineScopeError),
    KeyCodec(mako_storage::KeyCodecError),
    Storage(StorageError),
    Document(DocumentCodecError),
    Json(serde_json::Error),
}

impl TrustedQueryError {
    #[must_use]
    pub const fn stable_code(&self) -> &'static str {
        match self {
            Self::RequiredIndex { .. } => "document_query_requires_index",
            Self::InvalidCursor => "document_query_cursor_invalid",
            Self::LimitTooLarge { .. } => "document_query_limit_invalid",
            _ => "document_query_invalid",
        }
    }
}

impl fmt::Display for TrustedQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequiredIndex { shape } => write!(
                formatter,
                "query requires an active index with equality fields {:?}, range field {:?}, and sort {:?}",
                shape.equality_fields, shape.range_field, shape.sort
            ),
            Self::InvalidField(field) => write!(formatter, "invalid query field {field}"),
            Self::DuplicatePredicateField(field) => {
                write!(formatter, "query field {field} has more than one predicate")
            }
            Self::MultipleRangePredicates => {
                formatter.write_str("only one bounded range predicate is supported")
            }
            Self::InvalidRangeBounds => formatter.write_str("query range bounds are invalid"),
            Self::LimitTooLarge { requested, maximum } => {
                write!(
                    formatter,
                    "query limit {requested} exceeds maximum {maximum}"
                )
            }
            Self::InvalidCursor => formatter.write_str("query cursor is invalid for this plan"),
            Self::CorruptIndexEntry => formatter.write_str("index entry is corrupt"),
            Self::Index(error) => error.fmt(formatter),
            Self::Scope(error) => error.fmt(formatter),
            Self::KeyCodec(error) => error.fmt(formatter),
            Self::Storage(error) => error.fmt(formatter),
            Self::Document(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid query encoding: {error}"),
        }
    }
}

impl Error for TrustedQueryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Index(error) => Some(error),
            Self::Scope(error) => Some(error),
            Self::KeyCodec(error) => Some(error),
            Self::Storage(error) => Some(error),
            Self::Document(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<IndexError> for TrustedQueryError {
    fn from(error: IndexError) -> Self {
        Self::Index(error)
    }
}

impl From<crate::DocumentEngineScopeError> for TrustedQueryError {
    fn from(error: crate::DocumentEngineScopeError) -> Self {
        Self::Scope(error)
    }
}

impl From<mako_storage::KeyCodecError> for TrustedQueryError {
    fn from(error: mako_storage::KeyCodecError) -> Self {
        Self::KeyCodec(error)
    }
}

impl From<StorageError> for TrustedQueryError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<DocumentCodecError> for TrustedQueryError {
    fn from(error: DocumentCodecError) -> Self {
        Self::Document(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use futures::executor::block_on;
    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentValidator, IndexDefinition, IndexField, IndexKind, MutationId,
        MutationInput, PrimaryKeyDefinition, SchemaCompatibility, SchemaVersion,
    };

    #[test]
    fn primary_key_and_compound_range_queries_page_deterministically() {
        block_on(async {
            let (scoped, sequencer, validator) = setup();
            let name = IndexName::parse("by-team-score").expect("name");
            let version = IndexVersion::new(1).expect("version");
            scoped
                .create_index(
                    IndexDefinition::new_building(
                        CollectionId::parse("todos").expect("collection"),
                        name.clone(),
                        version,
                        IndexKind::NonUnique,
                        [
                            IndexField::ascending("team").expect("field"),
                            IndexField::descending("score").expect("field"),
                        ],
                    )
                    .expect("index"),
                    Durability::Memory,
                )
                .await
                .expect("create index");
            scoped
                .update_index_catalog(&name, version, Durability::Memory, |definition| {
                    definition.activate()
                })
                .await
                .expect("activate");

            let mut lease = sequencer
                .lease(NonZeroU64::new(6).expect("non-zero"))
                .await
                .expect("lease");
            for (id, team, score) in [
                ("blue-1", "blue", 1),
                ("blue-2", "blue", 2),
                ("blue-3", "blue", 3),
                ("blue-4", "blue", 4),
                ("red-9", "red", 9),
            ] {
                let position = lease.issue().expect("position");
                scoped
                    .create_document(MutationInput {
                        mutation_id: MutationId::parse(format!("create-{id}")).expect("mutation"),
                        commit_position: CommitPosition::new(position).expect("position"),
                        document: validator
                            .validate_create(json!({"id": id, "team": team, "score": score}))
                            .expect("document"),
                        durability: Durability::Memory,
                    })
                    .await
                    .expect("create");
            }

            let primary = scoped
                .trusted_query(&TrustedQuery::by_primary_key(
                    DocumentId::parse("blue-3").expect("id"),
                ))
                .await
                .expect("primary query");
            assert_eq!(primary.documents()[0].primary_key().as_str(), "blue-3");

            let indexed = IndexedQuery::new(
                [
                    QueryPredicate::equal("team", IndexValue::String("blue".to_owned()))
                        .expect("equality"),
                    QueryPredicate::bounded_range(
                        "score",
                        QueryBound::inclusive(IndexValue::number("1").expect("number")),
                        QueryBound::inclusive(IndexValue::number("4").expect("number")),
                    )
                    .expect("range"),
                ],
                [QuerySort::descending("score").expect("sort")],
                NonZeroUsize::new(2).expect("non-zero"),
            )
            .expect("query");
            assert!(matches!(
                scoped
                    .plan_trusted_query(&TrustedQuery::indexed(indexed.clone()))
                    .await
                    .expect("plan"),
                QueryPlan::Index {
                    equality_fields: 1,
                    range_field: Some(ref field),
                    ..
                } if field == "score"
            ));

            let filtered = scoped
                .trusted_query_authorized(
                    &TrustedQuery::indexed(indexed.clone()),
                    &EvenScoreAuthorizer,
                )
                .await
                .expect("filtered page");
            assert_eq!(ids(&filtered), ["blue-4", "blue-2"]);

            let stable_snapshot = scoped.snapshot().await.expect("query snapshot");
            let first = stable_snapshot
                .trusted_query(&TrustedQuery::indexed(indexed.clone()))
                .await
                .expect("first page");
            assert_eq!(ids(&first), ["blue-4", "blue-3"]);
            let concurrent_position = lease.issue().expect("position");
            scoped
                .create_document(MutationInput {
                    mutation_id: MutationId::parse("create-blue-2b").expect("mutation"),
                    commit_position: CommitPosition::new(concurrent_position).expect("position"),
                    document: validator
                        .validate_create(json!({"id": "blue-2b", "team": "blue", "score": 2}))
                        .expect("document"),
                    durability: Durability::Memory,
                })
                .await
                .expect("concurrent create");
            let second = stable_snapshot
                .trusted_query(&TrustedQuery::indexed(
                    indexed.with_cursor(first.next_cursor().expect("cursor").clone()),
                ))
                .await
                .expect("second page");
            assert_eq!(ids(&second), ["blue-2", "blue-1"]);
            assert!(second.next_cursor().is_none());

            let lower_only = IndexedQuery::new(
                [
                    QueryPredicate::equal("team", IndexValue::String("blue".to_owned()))
                        .expect("equality"),
                    QueryPredicate::greater_than_or_equal(
                        "score",
                        IndexValue::number("3").expect("number"),
                    )
                    .expect("lower range"),
                ],
                [QuerySort::descending("score").expect("sort")],
                NonZeroUsize::new(10).expect("non-zero"),
            )
            .expect("query");
            assert_eq!(
                ids(&scoped
                    .trusted_query(&TrustedQuery::indexed(lower_only))
                    .await
                    .expect("lower-only query")),
                ["blue-4", "blue-3"]
            );

            let upper_only = IndexedQuery::new(
                [
                    QueryPredicate::equal("team", IndexValue::String("blue".to_owned()))
                        .expect("equality"),
                    QueryPredicate::less_than("score", IndexValue::number("3").expect("number"))
                        .expect("upper range"),
                ],
                [QuerySort::descending("score").expect("sort")],
                NonZeroUsize::new(10).expect("non-zero"),
            )
            .expect("query");
            assert_eq!(
                ids(&scoped
                    .trusted_query(&TrustedQuery::indexed(upper_only))
                    .await
                    .expect("upper-only query")),
                ["blue-2", "blue-2b", "blue-1"]
            );
        });
    }

    #[test]
    fn unbounded_unsupported_and_cursor_mismatch_queries_fail_closed() {
        block_on(async {
            let (scoped, _, _) = setup();
            let unbounded = IndexedQuery::new(
                [],
                [QuerySort::ascending("team").expect("sort")],
                NonZeroUsize::new(10).expect("non-zero"),
            )
            .expect("query");
            let error = scoped
                .plan_trusted_query(&TrustedQuery::indexed(unbounded))
                .await
                .expect_err("unbounded query must fail");
            assert_eq!(error.stable_code(), "document_query_requires_index");
            assert!(matches!(error, TrustedQueryError::RequiredIndex { .. }));

            let unsupported = IndexedQuery::new(
                [
                    QueryPredicate::equal("team", IndexValue::String("blue".to_owned()))
                        .expect("predicate"),
                ],
                [],
                NonZeroUsize::new(10).expect("non-zero"),
            )
            .expect("query");
            assert_eq!(
                scoped
                    .plan_trusted_query(&TrustedQuery::indexed(unsupported))
                    .await
                    .expect_err("missing index")
                    .stable_code(),
                "document_query_requires_index"
            );
        });
    }

    fn ids(page: &QueryPage) -> Vec<&str> {
        page.documents()
            .iter()
            .map(|document| document.primary_key().as_str())
            .collect()
    }

    struct EvenScoreAuthorizer;

    impl DocumentReadAuthorizer for EvenScoreAuthorizer {
        fn authorize_read(
            &self,
            context: ReadAuthorizationContext<'_>,
        ) -> crate::MutationAuthorizationDecision {
            if context
                .document()
                .body()
                .get("score")
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|score| score % 2 == 0)
            {
                crate::MutationAuthorizationDecision::allow("test_visible")
            } else {
                crate::MutationAuthorizationDecision::deny("test_hidden")
            }
        }
    }

    fn setup() -> (
        ScopedCollectionEngine,
        crate::EnvironmentSequencer,
        DocumentValidator,
    ) {
        let adapter = Arc::new(MemoryAdapter::new());
        let engine = DocumentEngine::new(adapter);
        let tenant = TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        );
        let scoped = engine
            .scope_collection(
                &tenant,
                CollectionScope::new(
                    tenant.clone(),
                    CollectionId::parse("todos").expect("collection"),
                ),
            )
            .expect("scope");
        let sequencer = engine
            .scope_sequencer(&tenant, &tenant, Durability::Memory)
            .expect("sequencer");
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "team": {"type": "string"},
                    "score": {"type": "integer"}
                },
                "required": ["id", "team", "score"],
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        let validator = DocumentValidator::compile(&metadata).expect("validator");
        (scoped, sequencer, validator)
    }
}
