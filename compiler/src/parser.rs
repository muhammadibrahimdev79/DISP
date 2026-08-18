use crate::ast::{
    AssignmentOperator, BinaryOperator, BindingKind, Block, Capability, CapabilityUse, ClosureBody,
    DataConstraintDeclaration, DataConstraintKind, DataOrder, EnumDeclaration, Expr, Expression,
    ExternalAbi, ExternalFunction, FieldDeclaration, Function, FunctionSignature, GenericParameter,
    Implementation, ImportDeclaration, ImportItem, MatchArm, ModuleDeclaration, Parameter, Pattern,
    Program, Spanned, Statement, StructDeclaration, StructFieldValue, StructPatternField,
    TraitDeclaration, TypeName, TypeQualifier, UnaryOperator, VariantDeclaration,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Position, Span};
use crate::lexer::{Token, TokenKind};
use crate::limits;

pub struct Parser {
    tokens: Vec<Token>,
    current: usize,
    recursion_depth: usize,
    suppress_try: bool,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        let tokens = if tokens.is_empty() {
            vec![Token {
                kind: TokenKind::Eof,
                span: Span::point(1, 1),
            }]
        } else {
            tokens
        };
        Self {
            tokens,
            current: 0,
            recursion_depth: 0,
            suppress_try: false,
        }
    }

    pub fn parse(&mut self) -> Result<Program, Diagnostic> {
        let module = if self.check(&TokenKind::Module) {
            Some(self.parse_module_declaration()?)
        } else {
            None
        };
        let mut imports = Vec::new();
        let mut public_items = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut traits = Vec::new();
        let mut implementations = Vec::new();
        let mut functions = Vec::new();
        while !self.check(&TokenKind::Eof) {
            let public = self.match_token(&TokenKind::Pub);
            if self.check(&TokenKind::Use) {
                imports.push(self.parse_import(public)?);
                continue;
            }
            if self.check(&TokenKind::Struct) || self.check(&TokenKind::Data) {
                let declaration = self.parse_struct(self.check(&TokenKind::Data))?;
                if public {
                    public_items.push(Spanned {
                        node: declaration.name.clone(),
                        span: declaration.name_span,
                    });
                }
                structs.push(declaration);
            } else if self.check(&TokenKind::Enum) {
                let declaration = self.parse_enum()?;
                if public {
                    public_items.push(Spanned {
                        node: declaration.name.clone(),
                        span: declaration.name_span,
                    });
                }
                enums.push(declaration);
            } else if self.check(&TokenKind::Fn) || self.check(&TokenKind::Async) {
                let declaration = self.parse_function()?;
                if public {
                    public_items.push(Spanned {
                        node: declaration.name.clone(),
                        span: declaration.name_span,
                    });
                }
                functions.push(declaration);
            } else if self.check(&TokenKind::Export) {
                if public {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Parse,
                        "`export C` already defines external visibility and cannot be combined with `pub`",
                        self.previous().span,
                    ));
                }
                if self.export_declaration_is_struct() {
                    structs.push(self.parse_export_struct()?);
                } else {
                    functions.push(self.parse_export_function()?);
                }
            } else if self.check(&TokenKind::Extern) {
                let declarations = self.parse_extern_block()?;
                if public {
                    public_items.extend(declarations.iter().map(|declaration| Spanned {
                        node: declaration.name.clone(),
                        span: declaration.name_span,
                    }));
                }
                functions.extend(declarations);
            } else if self.check(&TokenKind::Trait) {
                let declaration = self.parse_trait()?;
                if public {
                    public_items.push(Spanned {
                        node: declaration.name.clone(),
                        span: declaration.name_span,
                    });
                }
                traits.push(declaration);
            } else if self.check(&TokenKind::Impl) {
                if public {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Parse,
                        "`pub` applies to named declarations, not implementation blocks",
                        self.previous().span,
                    ));
                }
                implementations.push(self.parse_implementation()?);
            } else {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "expected a top-level declaration or `use` import",
                    self.peek().span,
                ));
            }
        }
        Ok(Program {
            source_files: vec![],
            module,
            imports,
            public_items,
            structs,
            enums,
            traits,
            implementations,
            functions,
        })
    }

    fn parse_module_declaration(&mut self) -> Result<ModuleDeclaration, Diagnostic> {
        let start = self.expect(TokenKind::Module, "expected `module`")?.span;
        let path = self.parse_module_path("expected module name")?;
        let end = path.last().map_or(start, |part| part.span);
        self.match_token(&TokenKind::Semicolon);
        Ok(ModuleDeclaration {
            path,
            span: start.through(end),
        })
    }

    fn parse_import(&mut self, public: bool) -> Result<ImportDeclaration, Diagnostic> {
        let start = self.expect(TokenKind::Use, "expected `use`")?.span;
        let mut path = Vec::new();
        let (first, first_span) = self.expect_identifier("expected module name after `use`")?;
        path.push(Spanned {
            node: first,
            span: first_span,
        });
        let mut items = None;
        while self.match_token(&TokenKind::Dot) {
            if self.match_token(&TokenKind::LeftBrace) {
                let mut selected = Vec::new();
                if !self.check(&TokenKind::RightBrace) {
                    loop {
                        let (name, span) = self.expect_identifier("expected imported item name")?;
                        let (alias, alias_span) = if self.match_token(&TokenKind::As) {
                            self.expect_identifier("expected import alias after `as`")?
                        } else {
                            (name.clone(), span)
                        };
                        selected.push(ImportItem {
                            name,
                            name_span: span,
                            alias,
                            alias_span,
                            span: span.through(alias_span),
                        });
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                if selected.is_empty() {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Parse,
                        "an import item list cannot be empty",
                        self.peek().span,
                    ));
                }
                self.expect(TokenKind::RightBrace, "expected `}` after imported items")?;
                items = Some(selected);
                break;
            }
            let (part, span) = self.expect_identifier("expected module path component")?;
            path.push(Spanned { node: part, span });
        }
        let end = self.previous().span;
        self.match_token(&TokenKind::Semicolon);
        Ok(ImportDeclaration {
            path,
            items,
            public,
            span: start.through(end),
        })
    }

    fn parse_module_path(
        &mut self,
        message: &'static str,
    ) -> Result<Vec<Spanned<String>>, Diagnostic> {
        let mut path = Vec::new();
        let (first, span) = self.expect_identifier(message)?;
        path.push(Spanned { node: first, span });
        while self.match_token(&TokenKind::Dot) {
            let (part, span) = self.expect_identifier("expected module path component")?;
            path.push(Spanned { node: part, span });
        }
        Ok(path)
    }

    fn parse_struct(&mut self, data: bool) -> Result<StructDeclaration, Diagnostic> {
        let start = self
            .expect(
                if data {
                    TokenKind::Data
                } else {
                    TokenKind::Struct
                },
                if data {
                    "expected `data`"
                } else {
                    "expected `struct`"
                },
            )?
            .span;
        let (name, name_span) = self.expect_identifier(if data {
            "expected data schema name"
        } else {
            "expected struct name"
        })?;
        let generics = self.parse_generic_parameters()?;
        self.expect(
            TokenKind::LeftBrace,
            if data {
                "expected `{` after data schema name"
            } else {
                "expected `{` after struct name"
            },
        )?;
        let mut fields = Vec::new();
        let mut data_constraints = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if data && self.check_identifier("constraint") && !self.check_next(&TokenKind::Colon) {
                data_constraints.push(self.parse_data_constraint()?);
                continue;
            }
            let (field_name, field_span) = self.expect_identifier("expected field name")?;
            self.expect(TokenKind::Colon, "expected `:` after field name")?;
            let ty = self.parse_type_name()?;
            let mut primary = false;
            let mut unique = false;
            let mut indexed = false;
            let mut migration_from = None;
            let mut migration_default = None;
            if data {
                loop {
                    match &self.peek().kind {
                        TokenKind::Identifier(modifier) if modifier == "primary" => {
                            if primary {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Parse,
                                    "duplicate `primary` data-field constraint",
                                    self.peek().span,
                                ));
                            }
                            primary = true;
                            self.advance();
                        }
                        TokenKind::Identifier(modifier) if modifier == "unique" => {
                            if unique {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Parse,
                                    "duplicate `unique` data-field constraint",
                                    self.peek().span,
                                ));
                            }
                            unique = true;
                            self.advance();
                        }
                        TokenKind::Identifier(modifier) if modifier == "index" => {
                            if indexed {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Parse,
                                    "duplicate `index` data-field constraint",
                                    self.peek().span,
                                ));
                            }
                            indexed = true;
                            self.advance();
                        }
                        TokenKind::Identifier(modifier) if modifier == "from" => {
                            if migration_from.is_some() {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Parse,
                                    "duplicate `from` data-field migration",
                                    self.peek().span,
                                ));
                            }
                            self.advance();
                            self.expect(TokenKind::LeftParen, "expected `(` after `from`")?;
                            let (name, span) =
                                self.expect_identifier("expected prior field name")?;
                            self.expect(
                                TokenKind::RightParen,
                                "expected `)` after prior field name",
                            )?;
                            migration_from = Some(Spanned { node: name, span });
                        }
                        TokenKind::Identifier(modifier) if modifier == "default" => {
                            if migration_default.is_some() {
                                return Err(Diagnostic::new(
                                    DiagnosticKind::Parse,
                                    "duplicate `default` data-field migration",
                                    self.peek().span,
                                ));
                            }
                            self.advance();
                            self.expect(TokenKind::LeftParen, "expected `(` after `default`")?;
                            migration_default = Some(self.parse_expression()?);
                            self.expect(
                                TokenKind::RightParen,
                                "expected `)` after migration default",
                            )?;
                        }
                        _ => break,
                    }
                }
            }
            fields.push(FieldDeclaration {
                name: field_name,
                name_span: field_span,
                ty,
                primary,
                unique,
                indexed,
                migration_from,
                migration_default,
            });
            self.match_token(&TokenKind::Comma);
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected `}` after struct fields")?
            .span;
        Ok(StructDeclaration {
            name,
            name_span,
            data,
            c_abi: false,
            generics,
            fields,
            data_constraints,
            span: start.through(end),
        })
    }

    fn parse_data_constraint(&mut self) -> Result<DataConstraintDeclaration, Diagnostic> {
        let start = self.advance().span;
        let (name, name_span) = self.expect_identifier("expected constraint name")?;
        self.expect(TokenKind::Colon, "expected `:` after constraint name")?;
        let (kind, kind_span) = self.expect_identifier("expected `unique` or `index`")?;
        let kind = match kind.as_str() {
            "unique" => DataConstraintKind::Unique,
            "index" => DataConstraintKind::Index,
            _ => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "expected `unique` or `index` constraint kind",
                    kind_span,
                ));
            }
        };
        self.expect(TokenKind::LeftParen, "expected `(` after constraint kind")?;
        let mut fields = Vec::new();
        loop {
            let (field, span) = self.expect_identifier("expected constrained field name")?;
            fields.push(Spanned { node: field, span });
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        let end = self
            .expect(
                TokenKind::RightParen,
                "expected `)` after constrained fields",
            )?
            .span;
        self.match_token(&TokenKind::Comma);
        Ok(DataConstraintDeclaration {
            name,
            name_span,
            kind,
            fields,
            span: start.through(end),
        })
    }

    fn parse_enum(&mut self) -> Result<EnumDeclaration, Diagnostic> {
        let start = self.expect(TokenKind::Enum, "expected `enum`")?.span;
        let (name, name_span) = self.expect_identifier("expected enum name")?;
        let generics = self.parse_generic_parameters()?;
        self.expect(TokenKind::LeftBrace, "expected `{` after enum name")?;
        let mut variants = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            let (variant_name, variant_span) = self.expect_identifier("expected variant name")?;
            let mut payload = Vec::new();
            if self.match_token(&TokenKind::LeftParen) {
                if !self.check(&TokenKind::RightParen) {
                    loop {
                        payload.push(self.parse_type_name()?);
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(TokenKind::RightParen, "expected `)` after variant payload")?;
            }
            variants.push(VariantDeclaration {
                name: variant_name,
                name_span: variant_span,
                payload,
            });
            self.match_token(&TokenKind::Comma);
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected `}` after enum variants")?
            .span;
        Ok(EnumDeclaration {
            name,
            name_span,
            generics,
            variants,
            span: start.through(end),
        })
    }

    fn parse_trait(&mut self) -> Result<TraitDeclaration, Diagnostic> {
        let start = self.expect(TokenKind::Trait, "expected `trait`")?.span;
        let (name, name_span) = self.expect_identifier("expected trait name")?;
        let generics = self.parse_generic_parameters()?;
        self.expect(TokenKind::LeftBrace, "expected `{` after trait name")?;
        let mut associated_types = Vec::new();
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if self.match_token(&TokenKind::Type) {
                let (associated, span) = self.expect_identifier("expected associated type name")?;
                associated_types.push((associated, span));
                self.match_token(&TokenKind::Semicolon);
            } else {
                methods.push(self.parse_function_signature()?);
            }
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected `}` after trait")?
            .span;
        Ok(TraitDeclaration {
            name,
            name_span,
            generics,
            associated_types,
            methods,
            span: start.through(end),
        })
    }

    fn parse_function_signature(&mut self) -> Result<FunctionSignature, Diagnostic> {
        let asynchronous = self.match_token(&TokenKind::Async);
        let start = self
            .expect(TokenKind::Fn, "expected trait method or associated type")?
            .span;
        let (name, name_span) = self.expect_identifier("expected method name")?;
        let generics = self.parse_generic_parameters()?;
        let parameters = self.parse_parameters()?;
        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };
        let capabilities = self.parse_capabilities()?;
        self.match_token(&TokenKind::Semicolon);
        self.match_token(&TokenKind::Comma);
        let end = capabilities
            .as_ref()
            .and_then(|uses| uses.last().map(|item| item.span))
            .or_else(|| return_type.as_ref().map(|ty| ty.span))
            .unwrap_or(name_span);
        Ok(FunctionSignature {
            asynchronous,
            name,
            name_span,
            generics,
            parameters,
            return_type,
            capabilities,
            span: start.through(end),
        })
    }

    fn parse_implementation(&mut self) -> Result<Implementation, Diagnostic> {
        let start = self.expect(TokenKind::Impl, "expected `impl`")?.span;
        let generics = self.parse_generic_parameters()?;
        let first = self.parse_type_name()?;
        let (trait_name, target) = if self.match_token(&TokenKind::For) {
            (Some(first), self.parse_type_name()?)
        } else {
            (None, first)
        };
        self.expect(
            TokenKind::LeftBrace,
            "expected `{` after implementation target",
        )?;
        let mut associated_types = Vec::new();
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            if self.match_token(&TokenKind::Type) {
                let (name, span) = self.expect_identifier("expected associated type name")?;
                self.expect(
                    TokenKind::Equal,
                    "expected `=` in associated type definition",
                )?;
                let ty = self.parse_type_name()?;
                associated_types.push((name, ty, span));
                self.match_token(&TokenKind::Semicolon);
            } else {
                methods.push(self.parse_function()?);
            }
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected `}` after implementation")?
            .span;
        Ok(Implementation {
            generics,
            trait_name,
            target,
            associated_types,
            methods,
            span: start.through(end),
        })
    }

    fn parse_parameters(&mut self) -> Result<Vec<Parameter>, Diagnostic> {
        self.expect(TokenKind::LeftParen, "expected `(`")?;
        let mut parameters = Vec::new();
        if !self.check(&TokenKind::RightParen) {
            loop {
                if self.match_token(&TokenKind::And) {
                    let start = self.previous().span;
                    let mutable = self.match_token(&TokenKind::Mut);
                    let (receiver, name_span) =
                        self.expect_identifier("expected `self` after receiver `&`")?;
                    if receiver != "self" {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Parse,
                            "receiver shorthand must be `&self` or `&mut self`",
                            name_span,
                        ));
                    }
                    parameters.push(Parameter {
                        name: receiver,
                        name_span,
                        ty: TypeName {
                            name: "Self".into(),
                            arguments: vec![],
                            qualifier: if mutable {
                                TypeQualifier::MutableReference
                            } else {
                                TypeQualifier::SharedReference
                            },
                            span: start.through(name_span),
                        },
                    });
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                    continue;
                }
                let (name, name_span) = self.expect_identifier("expected parameter name")?;
                if name == "self" && !self.check(&TokenKind::Colon) {
                    parameters.push(Parameter {
                        name,
                        name_span,
                        ty: TypeName {
                            name: "Self".into(),
                            arguments: vec![],
                            qualifier: TypeQualifier::Owned,
                            span: name_span,
                        },
                    });
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                    continue;
                }
                self.expect(TokenKind::Colon, "expected `:` after parameter name")?;
                parameters.push(Parameter {
                    name,
                    name_span,
                    ty: self.parse_type_name()?,
                });
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RightParen, "expected `)` after parameters")?;
        Ok(parameters)
    }

    fn parse_function(&mut self) -> Result<Function, Diagnostic> {
        let asynchronous = self.match_token(&TokenKind::Async);
        let start = self.expect(TokenKind::Fn, "expected `fn`")?.span;
        let (name, name_span) = self.expect_identifier("expected function name")?;
        let generics = self.parse_generic_parameters()?;
        let parameters = self.parse_parameters()?;
        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };
        let capabilities = self.parse_capabilities()?;
        let body = if self.match_token(&TokenKind::Equal) {
            if return_type.is_none() {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "a concise function requires an explicit return type",
                    self.previous().span,
                ));
            }
            let value = self.parse_expression()?;
            self.match_token(&TokenKind::Semicolon);
            Block {
                span: value.span,
                statements: vec![Spanned {
                    span: value.span,
                    node: Statement::Return(Some(value)),
                }],
            }
        } else {
            self.parse_block()?
        };
        let span = start.through(body.span);
        Ok(Function {
            asynchronous,
            name,
            name_span,
            generics,
            parameters,
            return_type,
            capabilities,
            body,
            external: None,
            exported: false,
            span,
        })
    }

    fn parse_export_function(&mut self) -> Result<Function, Diagnostic> {
        let start = self.expect(TokenKind::Export, "expected `export`")?.span;
        let (abi, abi_span) = self.expect_identifier("expected ABI name after `export`")?;
        if abi != "C" {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                format!("unsupported export ABI `{abi}`"),
                abi_span,
            )
            .with_help("the stable exported ABI is written as `export C fn name(...)`"));
        }
        let mut function = self.parse_function()?;
        function.exported = true;
        function.span = start.through(function.span);
        Ok(function)
    }

    fn export_declaration_is_struct(&self) -> bool {
        matches!(
            (
                self.tokens.get(self.current + 1).map(|token| &token.kind),
                self.tokens.get(self.current + 2).map(|token| &token.kind),
            ),
            (Some(TokenKind::Identifier(abi)), Some(TokenKind::Struct)) if abi == "C"
        )
    }

    fn parse_export_struct(&mut self) -> Result<StructDeclaration, Diagnostic> {
        let start = self.expect(TokenKind::Export, "expected `export`")?.span;
        let (abi, abi_span) = self.expect_identifier("expected ABI name after `export`")?;
        if abi != "C" {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                format!("unsupported export ABI `{abi}`"),
                abi_span,
            )
            .with_help("a stable record layout is written as `export C struct Name { ... }`"));
        }
        let mut declaration = self.parse_struct(false)?;
        declaration.c_abi = true;
        declaration.span = start.through(declaration.span);
        Ok(declaration)
    }

    fn parse_extern_block(&mut self) -> Result<Vec<Function>, Diagnostic> {
        let start = self.expect(TokenKind::Extern, "expected `extern`")?.span;
        let (abi, abi_span) = self.expect_identifier("expected ABI name after `extern`")?;
        if abi != "C" {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                format!("unsupported external ABI `{abi}`"),
                abi_span,
            )
            .with_help("the defined foreign ABI is written as `extern C { ... }`"));
        }
        let library = if self.match_token(&TokenKind::LeftParen) {
            let token = self.advance();
            let TokenKind::String(library) = token.kind else {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "expected a library name string",
                    token.span,
                ));
            };
            self.expect(
                TokenKind::RightParen,
                "expected `)` after external library name",
            )?;
            Some(library)
        } else {
            None
        };
        self.expect(
            TokenKind::LeftBrace,
            "expected `{` after external ABI declaration",
        )?;
        let mut functions = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            let function_start = self
                .expect(TokenKind::Fn, "expected an external function declaration")?
                .span;
            let (name, name_span) = self.expect_identifier("expected external function name")?;
            let generics = self.parse_generic_parameters()?;
            let parameters = self.parse_parameters()?;
            let return_type = if self.match_token(&TokenKind::Arrow) {
                Some(self.parse_type_name()?)
            } else {
                None
            };
            let end = return_type
                .as_ref()
                .map_or_else(|| self.previous().span, |ty| ty.span);
            if self.check(&TokenKind::LeftBrace) || self.check(&TokenKind::Equal) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "an external function is a declaration and cannot have a DISP body",
                    self.peek().span,
                ));
            }
            self.match_token(&TokenKind::Semicolon);
            self.match_token(&TokenKind::Comma);
            functions.push(Function {
                asynchronous: false,
                name: name.clone(),
                name_span,
                generics,
                parameters,
                return_type,
                capabilities: Some(vec![CapabilityUse {
                    capability: Capability::Foreign,
                    span: function_start,
                }]),
                body: Block {
                    statements: vec![],
                    span: end,
                },
                external: Some(ExternalFunction {
                    abi: ExternalAbi::C,
                    library: library.clone(),
                    link_name: name.clone(),
                }),
                exported: false,
                span: function_start.through(end),
            });
        }
        self.expect(
            TokenKind::RightBrace,
            "expected `}` after external declarations",
        )?;
        if functions.is_empty() {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                "an external block must declare at least one function",
                start.through(abi_span),
            ));
        }
        Ok(functions)
    }

    fn parse_capabilities(&mut self) -> Result<Option<Vec<CapabilityUse>>, Diagnostic> {
        let TokenKind::Identifier(keyword) = &self.peek().kind else {
            return Ok(None);
        };
        if keyword != "uses" {
            return Ok(None);
        }
        let uses_span = self.advance().span;
        let (first, first_span) =
            self.expect_identifier("expected `Pure` or a capability after `uses`")?;
        if first == "Pure" {
            if self.match_token(&TokenKind::Comma) {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "`Pure` cannot be combined with capabilities",
                    uses_span.through(self.previous().span),
                ));
            }
            return Ok(Some(vec![]));
        }

        let mut capabilities = Vec::new();
        let mut current = Some((first, first_span));
        while let Some((name, span)) = current.take() {
            let capability = Capability::from_name(&name).ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Parse,
                    format!("unknown capability `{name}`"),
                    span,
                )
                .with_help(
                    "use `FileSystem`, `Network`, `Process`, `Foreign`, `RawMemory`, `DeviceIo`, `Timer`, `Random`, `Gpu`, `Ui`, or `Pure`",
                )
            })?;
            if capabilities
                .iter()
                .any(|item: &CapabilityUse| item.capability == capability)
            {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    format!("duplicate capability `{name}`"),
                    span,
                ));
            }
            capabilities.push(CapabilityUse { capability, span });
            if self.match_token(&TokenKind::Comma) {
                current = Some(self.expect_identifier("expected capability after `,`")?);
            }
        }
        Ok(Some(capabilities))
    }

    fn parse_generic_parameters(&mut self) -> Result<Vec<GenericParameter>, Diagnostic> {
        if !self.match_token(&TokenKind::Less) {
            return Ok(vec![]);
        }
        let mut parameters = Vec::new();
        loop {
            let (name, name_span) = self.expect_identifier("expected generic parameter")?;
            let mut constraints = Vec::new();
            if self.match_token(&TokenKind::Colon) {
                loop {
                    constraints.push(self.parse_type_name()?);
                    if !self.match_token(&TokenKind::Plus) {
                        break;
                    }
                }
            }
            parameters.push(GenericParameter {
                name,
                name_span,
                constraints,
            });
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::Greater, "expected `>` after generic parameters")?;
        Ok(parameters)
    }

    fn parse_type_name(&mut self) -> Result<TypeName, Diagnostic> {
        if self.match_token(&TokenKind::Fn) {
            let start = self.previous().span;
            self.expect(
                TokenKind::LeftParen,
                "expected `(` after `fn` in function type",
            )?;
            let mut arguments = Vec::new();
            if !self.check(&TokenKind::RightParen) {
                loop {
                    arguments.push(self.parse_type_name()?);
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(
                TokenKind::RightParen,
                "expected `)` after function parameters",
            )?;
            self.expect(TokenKind::Arrow, "expected `->` in function type")?;
            let result = self.parse_type_name()?;
            let end = result.span;
            arguments.push(result);
            return Ok(TypeName {
                name: "fn".into(),
                arguments,
                qualifier: TypeQualifier::Owned,
                span: start.through(end),
            });
        }
        if self.match_token(&TokenKind::LeftBracket) {
            let start = self.previous().span;
            let element = self.parse_type_name()?;
            if self.match_token(&TokenKind::RightBracket) {
                let end = self.previous().span;
                return Ok(TypeName {
                    name: "[]".into(),
                    arguments: vec![element],
                    qualifier: TypeQualifier::Owned,
                    span: start.through(end),
                });
            }
            self.expect(TokenKind::Semicolon, "expected `;` before array length")?;
            let length = match self.advance() {
                crate::lexer::Token {
                    kind: TokenKind::Integer(value),
                    span,
                } => usize::try_from(value).map_err(|_| {
                    Diagnostic::new(DiagnosticKind::Parse, "array length is too large", span)
                })?,
                token => {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Parse,
                        "expected constant array length",
                        token.span,
                    ));
                }
            };
            let end = self
                .expect(TokenKind::RightBracket, "expected `]` after array type")?
                .span;
            return Ok(TypeName {
                name: format!("[;{length}]"),
                arguments: vec![element],
                qualifier: TypeQualifier::Owned,
                span: start.through(end),
            });
        }
        if self.match_token(&TokenKind::And) {
            let start = self.previous().span;
            let mutable = self.match_token(&TokenKind::Mut);
            let mut ty = self.parse_type_name()?;
            ty.qualifier = if mutable {
                TypeQualifier::MutableReference
            } else {
                TypeQualifier::SharedReference
            };
            ty.span = start.through(ty.span);
            return Ok(ty);
        }
        let raw_mut = self.match_token(&TokenKind::Mut);
        let (mut name, start) = self.expect_identifier("expected type name")?;
        let mut arguments = Vec::new();
        let mut end = start;
        if self.match_token(&TokenKind::Dot) {
            if name != "Self" {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "Candidate 1 associated type projections must start with `Self`",
                    start,
                ));
            }
            let (associated, associated_span) =
                self.expect_identifier("expected associated type name after `Self.`")?;
            name.push('.');
            name.push_str(&associated);
            end = associated_span;
        }
        if self.match_token(&TokenKind::Less) {
            if name.contains('.') {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    "associated type projections cannot take type arguments",
                    end,
                ));
            }
            loop {
                arguments.push(self.parse_type_name()?);
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
            end = self.expect_type_close()?.span;
        }
        if raw_mut && name != "ptr" {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                "`mut` in a type is only valid as `mut ptr<T>`",
                start,
            ));
        }
        let qualifier = if raw_mut {
            TypeQualifier::RawMutPointer
        } else if name == "ptr" {
            TypeQualifier::RawConstPointer
        } else {
            TypeQualifier::Owned
        };
        Ok(TypeName {
            name,
            arguments,
            qualifier,
            span: start.through(end),
        })
    }

    fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        self.with_recursion(Self::parse_block_inner)
    }

    fn parse_block_inner(&mut self) -> Result<Block, Diagnostic> {
        let start = self.expect(TokenKind::LeftBrace, "expected `{`")?.span;
        let mut statements = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            statements.push(self.parse_statement()?);
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected `}` after block")?
            .span;
        Ok(Block {
            statements,
            span: start.through(end),
        })
    }

    fn parse_statement(&mut self) -> Result<Spanned<Statement>, Diagnostic> {
        if self.matches_any(&[TokenKind::Let, TokenKind::Var, TokenKind::Const]) {
            return self.parse_binding();
        }
        if self.match_token(&TokenKind::Return) {
            return self.parse_return();
        }
        if self.match_token(&TokenKind::If) {
            return self.parse_if();
        }
        if self.match_token(&TokenKind::While) {
            return self.parse_while();
        }
        if self.match_token(&TokenKind::For) {
            return self.parse_for();
        }
        if self.match_token(&TokenKind::Loop) {
            let start = self.previous().span;
            let body = self.parse_block()?;
            return Ok(Spanned {
                span: start.through(body.span),
                node: Statement::Loop(body),
            });
        }
        if self.match_token(&TokenKind::Unsafe) {
            let start = self.previous().span;
            let capabilities = self.parse_capabilities()?;
            let body = self.parse_block()?;
            return Ok(Spanned {
                span: start.through(body.span),
                node: Statement::Unsafe { capabilities, body },
            });
        }
        if self.match_token(&TokenKind::Break) {
            let span = self.previous().span;
            self.match_token(&TokenKind::Semicolon);
            return Ok(Spanned {
                node: Statement::Break,
                span,
            });
        }
        if self.match_token(&TokenKind::Continue) {
            let span = self.previous().span;
            self.match_token(&TokenKind::Semicolon);
            return Ok(Spanned {
                node: Statement::Continue,
                span,
            });
        }
        let expression = self.parse_expression()?;
        if is_assignment(&self.peek().kind) {
            let operator_token = self.advance();
            let operator = match operator_token.kind {
                TokenKind::Equal => AssignmentOperator::Assign,
                TokenKind::PlusEqual => AssignmentOperator::Add,
                TokenKind::MinusEqual => AssignmentOperator::Subtract,
                TokenKind::StarEqual => AssignmentOperator::Multiply,
                TokenKind::SlashEqual => AssignmentOperator::Divide,
                _ => unreachable!(),
            };
            let value = self.parse_expression()?;
            let span = expression.span.through(value.span);
            self.match_token(&TokenKind::Semicolon);
            if let Expression::Identifier(name) = &expression.node {
                return Ok(Spanned {
                    span,
                    node: Statement::Assignment {
                        name: name.clone(),
                        name_span: expression.span,
                        operator,
                        value,
                    },
                });
            }
            return Ok(Spanned {
                span,
                node: Statement::PlaceAssignment {
                    target: expression,
                    operator,
                    value,
                },
            });
        }
        let span = expression.span;
        self.match_token(&TokenKind::Semicolon);
        Ok(Spanned {
            node: Statement::Expression(expression),
            span,
        })
    }

    fn parse_binding(&mut self) -> Result<Spanned<Statement>, Diagnostic> {
        let keyword = self.advance();
        let kind = match keyword.kind {
            TokenKind::Let => BindingKind::Let,
            TokenKind::Var => BindingKind::Var,
            TokenKind::Const => BindingKind::Const,
            _ => unreachable!("binding parser called without a binding keyword"),
        };
        let (name, name_span) = self.expect_identifier("expected binding name")?;
        let annotation = if self.match_token(&TokenKind::Colon) {
            Some(self.parse_type_name()?)
        } else {
            None
        };
        let value = if self.match_token(&TokenKind::Equal) {
            Some(self.parse_expression()?)
        } else {
            None
        };
        if value.is_none() && annotation.is_none() {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                "an uninitialized binding requires a type annotation",
                name_span,
            ));
        }
        let span = keyword.span.through(value.as_ref().map_or(
            annotation.as_ref().map_or(name_span, |ty| ty.span),
            |value| value.span,
        ));
        self.match_token(&TokenKind::Semicolon);
        Ok(Spanned {
            node: Statement::Binding {
                kind,
                name,
                name_span,
                annotation,
                value,
            },
            span,
        })
    }

    fn parse_return(&mut self) -> Result<Spanned<Statement>, Diagnostic> {
        let start = self.previous().span;
        let value = if self.check(&TokenKind::Semicolon) || self.check(&TokenKind::RightBrace) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        let span = value
            .as_ref()
            .map_or(start, |value| start.through(value.span));
        self.match_token(&TokenKind::Semicolon);
        Ok(Spanned {
            node: Statement::Return(value),
            span,
        })
    }

    fn parse_if(&mut self) -> Result<Spanned<Statement>, Diagnostic> {
        let start = self.previous().span;
        let condition = self.parse_expression()?;
        let then_branch = self.parse_block()?;
        let else_branch = if self.match_token(&TokenKind::Else) {
            if self.match_token(&TokenKind::If) {
                let nested = self.parse_if()?;
                Some(Block {
                    span: nested.span,
                    statements: vec![nested],
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_branch.span, |branch| branch.span);
        Ok(Spanned {
            node: Statement::If {
                condition,
                then_branch,
                else_branch,
            },
            span: start.through(end),
        })
    }

    fn parse_while(&mut self) -> Result<Spanned<Statement>, Diagnostic> {
        let start = self.previous().span;
        let condition = self.parse_expression()?;
        let body = self.parse_block()?;
        Ok(Spanned {
            span: start.through(body.span),
            node: Statement::While { condition, body },
        })
    }

    fn parse_for(&mut self) -> Result<Spanned<Statement>, Diagnostic> {
        let start_span = self.previous().span;
        let (name, name_span) = self.expect_identifier("expected loop binding after `for`")?;
        self.expect(TokenKind::In, "expected `in` after loop binding")?;
        let start = self.parse_expression()?;
        let inclusive = if self.match_token(&TokenKind::RangeInclusive) {
            true
        } else if self.match_token(&TokenKind::Range) {
            false
        } else {
            let body = self.parse_block()?;
            return Ok(Spanned {
                span: start_span.through(body.span),
                node: Statement::ForEach {
                    name,
                    name_span,
                    iterable: start,
                    body,
                },
            });
        };
        let end = self.parse_expression()?;
        let body = self.parse_block()?;
        Ok(Spanned {
            span: start_span.through(body.span),
            node: Statement::For {
                name,
                name_span,
                start,
                end,
                inclusive,
                body,
            },
        })
    }

    fn parse_expression(&mut self) -> Result<Expr, Diagnostic> {
        self.with_recursion(Self::parse_or)
    }

    fn parse_binary(
        &mut self,
        operand: fn(&mut Self) -> Result<Expr, Diagnostic>,
        operators: &[(TokenKind, BinaryOperator)],
    ) -> Result<Expr, Diagnostic> {
        let mut expression = operand(self)?;
        let mut operator_count = 0usize;
        while let Some((_, operator)) = operators.iter().find(|(kind, _)| self.check(kind)) {
            if self.check(&TokenKind::Star)
                && self.peek().span.start.line > expression.span.end.line
                && self.looks_like_dereference_assignment()
            {
                break;
            }
            operator_count += 1;
            if operator_count > limits::MAX_OPERATOR_CHAIN {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    format!(
                        "operator chain exceeds the safety limit of {}",
                        limits::MAX_OPERATOR_CHAIN
                    ),
                    self.peek().span,
                ));
            }
            self.advance();
            let right = operand(self)?;
            let span = expression.span.through(right.span);
            expression = Spanned {
                node: Expression::Binary {
                    left: Box::new(expression),
                    operator: *operator,
                    right: Box::new(right),
                },
                span,
            };
        }
        Ok(expression)
    }

    fn looks_like_dereference_assignment(&self) -> bool {
        let mut index = self.current + 1;
        if !matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Identifier(_))
        ) {
            return false;
        }
        index += 1;
        while matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Dot)
        ) {
            index += 1;
            if !matches!(
                self.tokens.get(index).map(|token| &token.kind),
                Some(TokenKind::Identifier(_))
            ) {
                return false;
            }
            index += 1;
        }
        self.tokens
            .get(index)
            .is_some_and(|token| is_assignment(&token.kind))
    }

    fn parse_or(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(Self::parse_and, &[(TokenKind::OrOr, BinaryOperator::Or)])
    }

    fn parse_and(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Self::parse_equality,
            &[(TokenKind::AndAnd, BinaryOperator::And)],
        )
    }

    fn parse_equality(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Self::parse_comparison,
            &[
                (TokenKind::EqualEqual, BinaryOperator::Equal),
                (TokenKind::BangEqual, BinaryOperator::NotEqual),
            ],
        )
    }

    fn parse_comparison(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Self::parse_addition,
            &[
                (TokenKind::Less, BinaryOperator::Less),
                (TokenKind::LessEqual, BinaryOperator::LessEqual),
                (TokenKind::Greater, BinaryOperator::Greater),
                (TokenKind::GreaterEqual, BinaryOperator::GreaterEqual),
            ],
        )
    }

    fn parse_addition(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Self::parse_multiplication,
            &[
                (TokenKind::Plus, BinaryOperator::Add),
                (TokenKind::Minus, BinaryOperator::Subtract),
            ],
        )
    }

    fn parse_multiplication(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_binary(
            Self::parse_unary,
            &[
                (TokenKind::Star, BinaryOperator::Multiply),
                (TokenKind::Slash, BinaryOperator::Divide),
                (TokenKind::Percent, BinaryOperator::Remainder),
            ],
        )
    }

    fn parse_unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.match_token(&TokenKind::Await) {
            let start = self.previous().span;
            let future = self.with_recursion(Self::parse_unary)?;
            return Ok(Spanned {
                span: start.through(future.span),
                node: Expression::Await(Box::new(future)),
            });
        }
        if self.match_token(&TokenKind::Spawn) {
            let start = self.previous().span;
            let task = self.with_recursion(Self::parse_unary)?;
            return Ok(Spanned {
                span: start.through(task.span),
                node: Expression::Spawn(Box::new(task)),
            });
        }
        if self.match_token(&TokenKind::Move) {
            let start = self.previous().span;
            if self.match_token(&TokenKind::Or) {
                return self.parse_closure(start, true, false);
            }
            if self.match_token(&TokenKind::OrOr) {
                return self.parse_closure(start, true, true);
            }
            let operand = self.with_recursion(Self::parse_unary)?;
            return Ok(Spanned {
                span: start.through(operand.span),
                node: Expression::Move(Box::new(operand)),
            });
        }
        if self.match_token(&TokenKind::And) {
            let start = self.previous().span;
            let mutable = self.match_token(&TokenKind::Mut);
            let target = self.with_recursion(Self::parse_unary)?;
            return Ok(Spanned {
                span: start.through(target.span),
                node: Expression::Borrow {
                    mutable,
                    target: Box::new(target),
                },
            });
        }
        if self.match_token(&TokenKind::Star) {
            let start = self.previous().span;
            let target = self.with_recursion(Self::parse_unary)?;
            return Ok(Spanned {
                span: start.through(target.span),
                node: Expression::Dereference(Box::new(target)),
            });
        }
        let operator = if self.match_token(&TokenKind::Minus) {
            Some(UnaryOperator::Negate)
        } else if self.match_token(&TokenKind::Bang) {
            Some(UnaryOperator::Not)
        } else {
            None
        };
        let Some(operator) = operator else {
            return self.parse_call();
        };
        let start = self.previous().span;
        let operand = self.with_recursion(Self::parse_unary)?;
        Ok(Spanned {
            span: start.through(operand.span),
            node: Expression::Unary {
                operator,
                operand: Box::new(operand),
            },
        })
    }

    fn parse_call(&mut self) -> Result<Expr, Diagnostic> {
        let mut expression = self.parse_primary()?;
        let mut call_count = 0usize;
        loop {
            if self.match_token(&TokenKind::LeftParen) {
                call_count += 1;
                if call_count > limits::MAX_CALL_CHAIN {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Parse,
                        format!(
                            "call chain exceeds the safety limit of {}",
                            limits::MAX_CALL_CHAIN
                        ),
                        self.previous().span,
                    ));
                }
                let mut arguments = Vec::new();
                let map_literal = matches!(
                    &expression.node,
                    Expression::FieldAccess { object, field, .. }
                        if field == "of" && matches!(&object.node, Expression::Identifier(name) if name == "Map")
                );
                if !self.check(&TokenKind::RightParen) {
                    loop {
                        arguments.push(self.parse_expression()?);
                        if map_literal {
                            self.expect(
                                TokenKind::Colon,
                                "expected `:` between Map key and value",
                            )?;
                            arguments.push(self.parse_expression()?);
                        }
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                let end = self
                    .expect(
                        TokenKind::RightParen,
                        "expected `)` after function arguments",
                    )?
                    .span;
                let span = expression.span.through(end);
                if let Expression::FieldAccess { object, field, .. } = &expression.node
                    && matches!(field.as_str(), "slice" | "slice_mut")
                {
                    if arguments.len() != 2 {
                        return Err(Diagnostic::new(
                            DiagnosticKind::Parse,
                            format!("`{field}` expects a start and end position"),
                            span,
                        ));
                    }
                    let range = Spanned {
                        node: Expression::Subslice {
                            object: object.clone(),
                            start: Box::new(arguments[0].clone()),
                            end: Box::new(arguments[1].clone()),
                        },
                        span,
                    };
                    expression = Spanned {
                        node: Expression::Borrow {
                            mutable: field == "slice_mut",
                            target: Box::new(range),
                        },
                        span,
                    };
                    continue;
                }
                expression = Spanned {
                    node: Expression::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                    span,
                };
            } else if self.match_token(&TokenKind::Dot) {
                let (field, field_span) = if self.check(&TokenKind::Spawn) {
                    let token = self.advance().clone();
                    ("spawn".into(), token.span)
                } else {
                    self.expect_identifier("expected field after `.`")?
                };
                let span = expression.span.through(field_span);
                expression = Spanned {
                    node: Expression::FieldAccess {
                        object: Box::new(expression),
                        field,
                        field_span,
                    },
                    span,
                };
            } else if self.match_token(&TokenKind::LeftBracket) {
                let first = self.parse_expression()?;
                let range = if self.match_token(&TokenKind::Range) {
                    Some(self.parse_expression()?)
                } else {
                    None
                };
                let end = self
                    .expect(TokenKind::RightBracket, "expected `]` after index")?
                    .span;
                let span = expression.span.through(end);
                expression = Spanned {
                    node: if let Some(end) = range {
                        Expression::Subslice {
                            object: Box::new(expression),
                            start: Box::new(first),
                            end: Box::new(end),
                        }
                    } else {
                        Expression::Index {
                            object: Box::new(expression),
                            index: Box::new(first),
                        }
                    },
                    span,
                };
            } else if !self.suppress_try && self.match_token(&TokenKind::Question) {
                let end = self.previous().span;
                let span = expression.span.through(end);
                expression = Spanned {
                    node: Expression::Try(Box::new(expression)),
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.advance();
        let span = token.span;
        let node = match token.kind {
            TokenKind::Integer(value) => Expression::Integer(value),
            TokenKind::Float(value) => Expression::Float(value),
            TokenKind::String(value) => Expression::String(value),
            TokenKind::Character(value) => Expression::Character(value),
            TokenKind::True => Expression::Bool(true),
            TokenKind::False => Expression::Bool(false),
            TokenKind::Data => return self.parse_data_expression(span),
            TokenKind::Or => return self.parse_closure(span, false, false),
            TokenKind::OrOr => return self.parse_closure(span, false, true),
            TokenKind::Identifier(name)
                if is_type_style(&name) && self.looks_like_struct_construct() =>
            {
                return self.parse_struct_construct(name, span);
            }
            TokenKind::Identifier(name) => Expression::Identifier(name),
            TokenKind::Match => return self.parse_match(span),
            TokenKind::LeftParen => {
                let expression = self.parse_expression()?;
                let end = self.expect(TokenKind::RightParen, "expected `)` after expression")?;
                return Ok(Spanned {
                    span: span.through(end.span),
                    node: expression.node,
                });
            }
            TokenKind::LeftBracket => {
                let mut values = Vec::new();
                if !self.check(&TokenKind::RightBracket) {
                    loop {
                        values.push(self.parse_expression()?);
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                let end =
                    self.expect(TokenKind::RightBracket, "expected `]` after array literal")?;
                return Ok(Spanned {
                    span: span.through(end.span),
                    node: Expression::Array(values),
                });
            }
            found => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    format!("expected expression, found {found:?}"),
                    span,
                ));
            }
        };
        Ok(Spanned { node, span })
    }

    fn parse_data_expression(&mut self, start: Span) -> Result<Expr, Diagnostic> {
        let (operation, operation_span) =
            self.expect_identifier("expected a DISP Data operation")?;
        match operation.as_str() {
            "memory" => Ok(Spanned {
                node: Expression::DataStore { path: None },
                span: start.through(operation_span),
            }),
            "open" => {
                let path = self.parse_data_part()?;
                Ok(Spanned {
                    span: start.through(path.span),
                    node: Expression::DataStore {
                        path: Some(Box::new(path)),
                    },
                })
            }
            "add" => self.parse_data_write(start, false),
            "save" => self.parse_data_write(start, true),
            "find" => self.parse_data_query(start),
            "remove" => self.parse_data_remove(start),
            _ => Err(Diagnostic::new(
                DiagnosticKind::Parse,
                format!("unknown DISP Data operation `{operation}`"),
                operation_span,
            )
            .with_help("use `data memory`, `data open`, `data add`, `data save`, `data find`, or `data remove`")),
        }
    }

    fn parse_data_write(&mut self, start: Span, replace: bool) -> Result<Expr, Diagnostic> {
        let value = self.parse_data_part()?;
        self.expect(TokenKind::In, "expected `in` and a data store")?;
        let store = self.parse_data_part()?;
        Ok(Spanned {
            span: start.through(store.span),
            node: Expression::DataWrite {
                value: Box::new(value),
                store: Box::new(store),
                replace,
            },
        })
    }

    fn parse_data_query(&mut self, start: Span) -> Result<Expr, Diagnostic> {
        let (schema, schema_span) =
            self.expect_identifier("expected a data schema after `find`")?;
        self.expect(TokenKind::In, "expected `in` and a data store")?;
        let store = self.parse_data_part()?;
        let predicate = if self.match_data_word("where") {
            Some(Box::new(self.parse_data_part()?))
        } else {
            None
        };
        let order = if self.match_data_word("order") {
            let order_start = self.previous().span;
            let key = self.parse_data_part()?;
            let descending = if self.match_data_word("descending") {
                true
            } else {
                self.match_data_word("ascending");
                false
            };
            let end = self.previous().span;
            Some(DataOrder {
                key: Box::new(key),
                descending,
                span: order_start.through(end),
            })
        } else {
            None
        };
        let limit = if self.match_data_word("limit") {
            Some(Box::new(self.parse_data_part()?))
        } else {
            None
        };
        let end = limit.as_ref().map_or_else(
            || order.as_ref().map_or(store.span, |item| item.span),
            |item| item.span,
        );
        Ok(Spanned {
            span: start.through(end),
            node: Expression::DataQuery {
                schema,
                schema_span,
                store: Box::new(store),
                predicate,
                order,
                limit,
            },
        })
    }

    fn parse_data_remove(&mut self, start: Span) -> Result<Expr, Diagnostic> {
        let (schema, schema_span) =
            self.expect_identifier("expected a data schema after `remove`")?;
        self.expect(TokenKind::In, "expected `in` and a data store")?;
        let store = self.parse_data_part()?;
        if !self.match_data_word("where") {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                "`remove` requires `where` so all rows cannot be deleted accidentally",
                self.peek().span,
            ));
        }
        let predicate = self.parse_data_part()?;
        Ok(Spanned {
            span: start.through(predicate.span),
            node: Expression::DataRemove {
                schema,
                schema_span,
                store: Box::new(store),
                predicate: Box::new(predicate),
            },
        })
    }

    fn parse_data_part(&mut self) -> Result<Expr, Diagnostic> {
        let previous = self.suppress_try;
        self.suppress_try = true;
        let result = self.parse_expression();
        self.suppress_try = previous;
        result
    }

    fn match_data_word(&mut self, expected: &str) -> bool {
        if matches!(&self.peek().kind, TokenKind::Identifier(value) if value == expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn parse_closure(
        &mut self,
        start: Span,
        move_captures: bool,
        parameters_closed: bool,
    ) -> Result<Expr, Diagnostic> {
        let mut parameters = Vec::new();
        if !parameters_closed {
            if !self.check(&TokenKind::Or) {
                loop {
                    let (name, name_span) =
                        self.expect_identifier("expected closure parameter name")?;
                    self.expect(
                        TokenKind::Colon,
                        "closure parameters require a type annotation",
                    )?;
                    parameters.push(Parameter {
                        name,
                        name_span,
                        ty: self.parse_type_name()?,
                    });
                    if !self.match_token(&TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.expect(TokenKind::Or, "expected `|` after closure parameters")?;
        }
        let return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };
        let body = if self.check(&TokenKind::LeftBrace) {
            ClosureBody::Block(self.parse_block()?)
        } else {
            ClosureBody::Expression(Box::new(self.parse_expression()?))
        };
        if matches!(body, ClosureBody::Block(_)) && return_type.is_none() {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                "a block closure requires an explicit return type",
                start,
            )
            .with_help("write `|arguments| -> ResultType { ... }`"));
        }
        let end = match &body {
            ClosureBody::Expression(expression) => expression.span,
            ClosureBody::Block(block) => block.span,
        };
        Ok(Spanned {
            span: start.through(end),
            node: Expression::Closure {
                move_captures,
                parameters,
                return_type,
                body,
            },
        })
    }

    fn parse_struct_construct(
        &mut self,
        name: String,
        name_span: Span,
    ) -> Result<Expr, Diagnostic> {
        self.expect(TokenKind::LeftBrace, "expected `{` in struct construction")?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            let (field_name, field_span) = self.expect_identifier("expected struct field name")?;
            let value = if self.match_token(&TokenKind::Colon) {
                self.parse_expression()?
            } else {
                Spanned {
                    node: Expression::Identifier(field_name.clone()),
                    span: field_span,
                }
            };
            fields.push(StructFieldValue {
                name: field_name,
                name_span: field_span,
                value,
            });
            self.match_token(&TokenKind::Comma);
        }
        let end = self
            .expect(
                TokenKind::RightBrace,
                "expected `}` after struct construction",
            )?
            .span;
        Ok(Spanned {
            node: Expression::StructConstruct {
                name,
                name_span,
                fields,
            },
            span: name_span.through(end),
        })
    }

    fn parse_match(&mut self, start: Span) -> Result<Expr, Diagnostic> {
        let value = self.parse_expression()?;
        self.expect(TokenKind::LeftBrace, "expected `{` after match value")?;
        let mut arms = Vec::new();
        while !self.check(&TokenKind::RightBrace) && !self.check(&TokenKind::Eof) {
            let pattern = self.parse_pattern()?;
            let guard = if self.match_token(&TokenKind::If) {
                Some(self.parse_expression()?)
            } else {
                None
            };
            self.expect(TokenKind::FatArrow, "expected `=>` after match pattern")?;
            let arm_value = self.parse_expression()?;
            let span = pattern.span.through(arm_value.span);
            arms.push(MatchArm {
                pattern,
                alternative_group: None,
                guard,
                value: arm_value,
                span,
            });
            self.match_token(&TokenKind::Comma);
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected `}` after match arms")?
            .span;
        Ok(Spanned {
            node: Expression::Match {
                value: Box::new(value),
                arms,
            },
            span: start.through(end),
        })
    }

    fn parse_pattern(&mut self) -> Result<Spanned<Pattern>, Diagnostic> {
        let first = self.parse_pattern_atom()?;
        if !self.match_token(&TokenKind::Or) {
            return Ok(first);
        }
        let start = first.span;
        let mut alternatives = vec![first];
        loop {
            alternatives.push(self.parse_pattern_atom()?);
            if !self.match_token(&TokenKind::Or) {
                break;
            }
        }
        let end = alternatives.last().unwrap().span;
        Ok(Spanned {
            node: Pattern::Or(alternatives),
            span: start.through(end),
        })
    }

    fn parse_pattern_atom(&mut self) -> Result<Spanned<Pattern>, Diagnostic> {
        let token = self.advance();
        let start = token.span;
        let node = match token.kind {
            TokenKind::Minus => {
                let magnitude = self.advance();
                let TokenKind::Integer(value) = magnitude.kind else {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Parse,
                        "expected integer literal after `-` in pattern",
                        magnitude.span,
                    ));
                };
                return Ok(Spanned {
                    node: Pattern::NegativeInteger(value),
                    span: start.through(magnitude.span),
                });
            }
            TokenKind::Integer(value) => Pattern::Integer(value),
            TokenKind::String(value) => Pattern::String(value),
            TokenKind::Character(value) => Pattern::Character(value),
            TokenKind::True => Pattern::Bool(true),
            TokenKind::False => Pattern::Bool(false),
            TokenKind::Identifier(name) if name == "_" => Pattern::Wildcard,
            TokenKind::Identifier(name) if is_type_style(&name) => {
                if self.match_token(&TokenKind::LeftBrace) {
                    let mut fields = Vec::new();
                    let mut rest = false;
                    while !self.check(&TokenKind::RightBrace) {
                        if self.match_token(&TokenKind::Range) {
                            rest = true;
                            self.match_token(&TokenKind::Comma);
                            break;
                        }
                        let (field_name, field_span) =
                            self.expect_identifier("expected field in struct pattern")?;
                        let pattern = if self.match_token(&TokenKind::Colon) {
                            self.parse_pattern()?
                        } else {
                            Spanned {
                                node: Pattern::Binding(field_name.clone()),
                                span: field_span,
                            }
                        };
                        fields.push(StructPatternField {
                            name: field_name,
                            name_span: field_span,
                            pattern,
                        });
                        if !self.match_token(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self
                        .expect(TokenKind::RightBrace, "expected `}` after struct pattern")?
                        .span;
                    return Ok(Spanned {
                        node: Pattern::Struct {
                            type_name: name,
                            fields,
                            rest,
                        },
                        span: start.through(end),
                    });
                }
                let (type_name, variant) = if self.match_token(&TokenKind::Dot) {
                    let (variant, _) = self.expect_identifier("expected variant after `.`")?;
                    (Some(name), variant)
                } else {
                    (None, name)
                };
                let mut arguments = Vec::new();
                let mut end = self.previous().span;
                if self.match_token(&TokenKind::LeftParen) {
                    if !self.check(&TokenKind::RightParen) {
                        loop {
                            arguments.push(self.parse_pattern()?);
                            if !self.match_token(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    end = self
                        .expect(TokenKind::RightParen, "expected `)` after variant pattern")?
                        .span;
                }
                return Ok(Spanned {
                    node: Pattern::Variant {
                        type_name,
                        variant,
                        arguments,
                    },
                    span: start.through(end),
                });
            }
            TokenKind::Identifier(name) => Pattern::Binding(name),
            found => {
                return Err(Diagnostic::new(
                    DiagnosticKind::Parse,
                    format!("expected pattern, found {found:?}"),
                    start,
                ));
            }
        };
        Ok(Spanned { node, span: start })
    }

    fn looks_like_struct_construct(&self) -> bool {
        if !self.check(&TokenKind::LeftBrace) {
            return false;
        }
        if self
            .tokens
            .get(self.current + 1)
            .is_some_and(|token| same_variant(&token.kind, &TokenKind::RightBrace))
        {
            return true;
        }
        self.tokens
            .get(self.current + 1)
            .is_some_and(|token| matches!(token.kind, TokenKind::Identifier(_)))
            && self.tokens.get(self.current + 2).is_some_and(|token| {
                same_variant(&token.kind, &TokenKind::Colon)
                    || same_variant(&token.kind, &TokenKind::Comma)
                    || same_variant(&token.kind, &TokenKind::RightBrace)
            })
    }

    fn expect(&mut self, expected: TokenKind, message: &str) -> Result<Token, Diagnostic> {
        if self.check(&expected) {
            return Ok(self.advance());
        }
        let token = self.peek();
        Err(Diagnostic::new(
            DiagnosticKind::Parse,
            format!("{message}; found {:?}", token.kind),
            token.span,
        ))
    }

    fn expect_type_close(&mut self) -> Result<Token, Diagnostic> {
        if self.check(&TokenKind::Greater) {
            return Ok(self.advance());
        }
        if self.check(&TokenKind::ShiftRight) {
            let span = self.peek().span;
            let middle = Position {
                line: span.start.line,
                column: span.start.column + 1,
            };
            self.tokens[self.current] = Token {
                kind: TokenKind::Greater,
                span: Span::new(middle, span.end),
            };
            return Ok(Token {
                kind: TokenKind::Greater,
                span: Span::new(span.start, middle),
            });
        }
        self.expect(TokenKind::Greater, "expected `>` after type arguments")
    }

    fn expect_identifier(&mut self, message: &str) -> Result<(String, Span), Diagnostic> {
        let token = self.advance();
        match token.kind {
            TokenKind::Identifier(name) => Ok((name, token.span)),
            found => Err(Diagnostic::new(
                DiagnosticKind::Parse,
                format!("{message}; found {found:?}"),
                token.span,
            )),
        }
    }

    fn matches_any(&self, expected: &[TokenKind]) -> bool {
        expected.iter().any(|kind| self.check(kind))
    }

    fn match_token(&mut self, expected: &TokenKind) -> bool {
        if !self.check(expected) {
            return false;
        }
        self.advance();
        true
    }

    fn check(&self, expected: &TokenKind) -> bool {
        same_variant(&self.peek().kind, expected)
    }

    fn check_next(&self, expected: &TokenKind) -> bool {
        let index = (self.current + 1).min(self.tokens.len().saturating_sub(1));
        same_variant(&self.tokens[index].kind, expected)
    }

    fn check_identifier(&self, expected: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Identifier(value) if value == expected)
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.current += 1;
        }
        token
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current.min(self.tokens.len().saturating_sub(1))]
    }

    fn previous(&self) -> &Token {
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn with_recursion<T>(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        // A recursive expression level crosses several precedence-parser frames. Keep this
        // below the point where the public `check_source` API can exhaust a normal 1 MiB host
        // thread stack; the CLI's larger driver stack is defense in depth, not a requirement.
        if self.recursion_depth >= limits::MAX_EXPRESSION_DEPTH {
            return Err(Diagnostic::new(
                DiagnosticKind::Parse,
                format!(
                    "expression nesting exceeds the limit of {}",
                    limits::MAX_EXPRESSION_DEPTH
                ),
                self.peek().span,
            ));
        }
        self.recursion_depth += 1;
        let result = parse(self);
        self.recursion_depth -= 1;
        result
    }
}

fn same_variant(left: &TokenKind, right: &TokenKind) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

fn is_assignment(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Equal
            | TokenKind::PlusEqual
            | TokenKind::MinusEqual
            | TokenKind::StarEqual
            | TokenKind::SlashEqual
    )
}

fn is_type_style(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dereference_assignment() {
        let tokens = crate::lexer::Lexer::new(
            "fn main(){ mutex=Mutex.new(0) guard=mutex.lock()\n*guard += 1 }",
        )
        .tokenize()
        .unwrap();
        Parser::new(tokens).parse().unwrap();
    }
    use crate::lexer::Lexer;

    fn parse(source: &str) -> Result<Program, Diagnostic> {
        Parser::new(Lexer::new(source).tokenize()?).parse()
    }

    #[test]
    fn parses_functions_parameters_returns_and_bindings() {
        let program = parse(
            "fn add(a: i32, b: i32) -> i32 { return a + b } fn main() { var x: i32 = 1 const y = 2 x += y }",
        )
        .expect("parser should succeed");
        assert_eq!(program.functions.len(), 2);
        assert_eq!(program.functions[0].parameters.len(), 2);
        assert_eq!(
            program.functions[0].return_type.as_ref().unwrap().name,
            "i32"
        );
        assert!(matches!(
            program.functions[1].body.statements[2].node,
            Statement::Assignment {
                operator: AssignmentOperator::Add,
                ..
            }
        ));
    }

    #[test]
    fn respects_precedence_and_unary_binding() {
        let program =
            parse("fn main() { let x = -1 + 2 * 3 == 5 || false }").expect("parser should succeed");
        let Statement::Binding { value, .. } = &program.functions[0].body.statements[0].node else {
            panic!("expected binding");
        };
        assert!(matches!(
            value.as_ref().unwrap().node,
            Expression::Binary {
                operator: BinaryOperator::Or,
                ..
            }
        ));
    }

    #[test]
    fn parses_control_flow() {
        let program = parse(
            "fn main() { var x = 0 while x < 3 { if x == 1 { x += 1 continue } else { x += 1 } } for i in 0..=2 { x += i } loop { break } }",
        )
        .expect("parser should succeed");
        assert!(matches!(
            program.functions[0].body.statements[1].node,
            Statement::While { .. }
        ));
        assert!(matches!(
            program.functions[0].body.statements[2].node,
            Statement::For {
                inclusive: true,
                ..
            }
        ));
        assert!(matches!(
            program.functions[0].body.statements[3].node,
            Statement::Loop(_)
        ));
    }

    #[test]
    fn malformed_input_returns_diagnostic() {
        for source in [
            "fn",
            "fn main(",
            "fn main() { let x =",
            "fn main() { if true",
        ] {
            assert!(parse(source).is_err(), "`{source}` should be rejected");
        }
    }

    #[test]
    fn rejects_pathological_nesting_without_panicking() {
        std::thread::Builder::new()
            .name("disp-parser-depth-test".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(rejects_pathological_nesting_inner)
            .unwrap()
            .join()
            .unwrap();
    }

    fn rejects_pathological_nesting_inner() {
        let source = format!(
            "fn main() {{ let x = {}1{} }}",
            "(".repeat(100),
            ")".repeat(100)
        );
        let error = parse(&source).expect_err("nesting limit should reject the source");
        assert!(error.message.contains("nesting"));

        let nested_blocks = format!(
            "fn main() {{ {} print(1) {} }}",
            "if true { ".repeat(100),
            "}".repeat(100)
        );
        let error =
            parse(&nested_blocks).expect_err("block nesting limit should reject the source");
        assert!(error.message.contains("nesting"));

        let chain = format!("fn main() {{ print({}) }}", vec!["1"; 300].join(" + "));
        let error = parse(&chain).expect_err("operator chain limit should reject the source");
        assert!(error.message.contains("operator chain"));
    }
}
